// SPDX-License-Identifier: Apache-2.0

// Runs Slang's driver in-process without printing: diagnostics are recorded
// from the diagnostic engine, and the few messages the driver prints
// instead of reporting (missing files, bad option values) are captured.
// See doc/design/slang-in-process.md, "Step 1".

#include "slang-sys/shim/shim.h"

#include <memory>
#include <mutex>
#include <string>
#include <string_view>
#include <vector>

#include "slang-sys/shim/design.h"
#include "slang-sys/src/lib.rs.h"
#include "slang/analysis/AnalysisManager.h"
#include "slang/ast/Compilation.h"
#include "slang/ast/symbols/CompilationUnitSymbols.h"
#include "slang/diagnostics/DiagnosticClient.h"
#include "slang/diagnostics/DiagnosticEngine.h"
#include "slang/diagnostics/DriverDiags.h"
#include "slang/diagnostics/TextDiagnosticClient.h"
#include "slang/driver/Driver.h"
#include "slang/numeric/SVInt.h"
#include "slang/syntax/SyntaxTree.h"
#include "slang/text/SourceManager.h"
#include "slang/util/OS.h"
#include "slang/util/VersionInfo.h"

namespace slang_sys {

namespace {

using namespace slang;

constexpr uint8_t NOTE = 0;
constexpr uint8_t WARNING = 1;
constexpr uint8_t ERROR = 2;
constexpr uint8_t FATAL = 3;

/// Options whose effect lives in `Driver::optionallyWriteDepFiles` or
/// `Driver::reportDiagnostics`, which the shim never calls: they would
/// silently do nothing, so they are rejected.
std::vector<FfiDiag> unsupportedOptions(const driver::Driver::Options& options) {
    struct Rejected {
        bool set;
        std::vector<std::string_view> names;
    };
    const Rejected table[] = {
        {options.allDepfile.has_value(), {"--Mall", "--all-deps"}},
        {options.includeDepfile.has_value(), {"--Minclude", "--include-deps"}},
        {options.moduleDepfile.has_value(), {"--Mmodule", "--module-deps"}},
        {options.depfileTarget.has_value(), {"--depfile-target"}},
        {options.diagJson.has_value(), {"--diag-json"}},
    };
    std::vector<FfiDiag> out;
    for (auto& entry : table) {
        if (!entry.set)
            continue;
        std::string names;
        for (auto name : entry.names) {
            if (!names.empty())
                names += "`/`";
            names += name;
        }
        FfiDiag d{};
        d.severity = FATAL;
        d.message = str("`" + names +
                        "` is not supported in-process: Sandstitch does not write dependency "
                        "or diagnostic files; use the dependency list Sandstitch records");
        out.push_back(std::move(d));
    }
    return out;
}

/// Records every diagnostic the engine reports, after the engine applied
/// warning options, pragmas and the error limit.
class Recorder : public DiagnosticClient {
public:
    Recorder(const SourceManager& sourceManager, std::vector<FfiDiag>& out) :
        sourceManager(sourceManager), out(out) {}

    void report(const ReportedDiagnostic& diag) override {
        FfiDiag d{};
        d.severity = severity(diag);
        d.code = str(toString(diag.originalDiagnostic.code));
        d.message = str(diag.formattedMessage);
        // The engine's location is the macro use site; the original
        // diagnostic's maps back to where the text is written.
        auto raw = diag.originalDiagnostic.location;
        if (raw.buffer() != SourceLocation::NoLocation.buffer()) {
            auto loc = sourceManager.getFullyOriginalLoc(raw);
            if (sourceManager.isFileLoc(loc)) {
                d.file = str(sourceManager.getRawFileName(loc.buffer()));
                d.line = uint32_t(sourceManager.getLineNumber(loc));
                d.column = uint32_t(sourceManager.getColumnNumber(loc));
            }
        }
        out.push_back(std::move(d));
    }

private:
    static uint8_t severity(const ReportedDiagnostic& diag) {
        // A bad command line stops the run before elaboration.
        if (diag.originalDiagnostic.code == diag::CommandLineError)
            return FATAL;
        switch (diag.severity) {
            case DiagnosticSeverity::Note:
                return NOTE;
            case DiagnosticSeverity::Warning:
                return WARNING;
            case DiagnosticSeverity::Error:
                return ERROR;
            default:
                return FATAL;
        }
    }

    const SourceManager& sourceManager;
    std::vector<FfiDiag>& out;
};

/// The messages the driver printed (`error: `, `warning: `, `  note: `
/// prefixes); lines without a prefix continue the previous message. Errors
/// are fatal: the driver prints them for problems that stop a run.
std::vector<FfiDiag> printedDiags(const std::string& text) {
    std::vector<FfiDiag> out;
    std::string_view rest = text;
    while (!rest.empty()) {
        auto end = rest.find('\n');
        auto line = rest.substr(0, end);
        rest = end == std::string_view::npos ? std::string_view{} : rest.substr(end + 1);
        std::optional<uint8_t> severity;
        for (auto [prefix, sev] : {std::pair{"error: "sv, FATAL}, std::pair{"warning: "sv, WARNING},
                                   std::pair{"  note: "sv, NOTE}}) {
            if (line.starts_with(prefix)) {
                severity = sev;
                line.remove_prefix(prefix.size());
                break;
            }
        }
        if (severity) {
            FfiDiag d{};
            d.severity = *severity;
            d.message = str(line);
            out.push_back(std::move(d));
        }
        else if (!out.empty() && !line.empty()) {
            std::string message(out.back().message);
            message += "\n";
            message += line;
            out.back().message = str(message);
        }
    }
    return out;
}

/// Options, loading and parsing print through `OS`'s process-wide capture,
/// so only one run may be in that stage at a time. Nothing after it prints.
std::mutex frontMutex;

} // namespace

rust::String library_version() {
    return str(std::to_string(VersionInfo::getMajor()) + "." +
               std::to_string(VersionInfo::getMinor()) + "." +
               std::to_string(VersionInfo::getPatch()));
}

FfiResponse run(const FfiRequest& request) {
    FfiResponse response{};
    std::vector<FfiDiag> recorded;
    std::unique_ptr<driver::Driver> driver;
    auto text = std::make_shared<TextDiagnosticClient>();
    text->showColors(false);

    std::vector<std::string> args{"slang"};
    for (auto& arg : request.args)
        args.emplace_back(std::string(arg));
    std::vector<const char*> argv;
    for (auto& arg : args)
        argv.push_back(arg.c_str());

    bool optionsOk = false;
    std::string printed;
    std::vector<FfiDiag> unsupported;
    {
        std::lock_guard lock(frontMutex);
        auto capture = OS::captureOutput();
        driver = std::make_unique<driver::Driver>();
        // The constructor enables colors when stderr is a terminal.
        driver->setTerminalColorsEnabled(false);
        driver->diagEngine.clearClients();
        driver->diagEngine.addClient(std::make_shared<Recorder>(driver->sourceManager, recorded));
        driver->diagEngine.addClient(text);
        driver->addStandardArgs();
        optionsOk = driver->parseCommandLine(int(argv.size()), argv.data()) &&
                    driver->processOptions();
        if (optionsOk) {
            unsupported = unsupportedOptions(driver->options);
            optionsOk = unsupported.empty();
        }
        // As `slang`: a source that fails to load is reported, and the run
        // goes on with the rest.
        if (optionsOk)
            (void)driver->parseAllSources();
        printed = OS::capturedStderr + OS::capturedStdout;
        OS::capturedStderr.clear();
        OS::capturedStdout.clear();
    }

    std::unique_ptr<ast::Compilation> compilation;
    if (optionsOk) {
        if (request.stage == 0) {
            Diagnostics diags;
            for (auto& tree : driver->sourceLoader.getLibraryMaps())
                diags.append_range(tree->diagnostics());
            for (auto& tree : driver->syntaxTrees)
                diags.append_range(tree->diagnostics());
            diags.sort(driver->sourceManager);
            driver->diagEngine.issue(diags);
        }
        else {
            compilation = driver->createCompilation();
            driver->diagEngine.issue(compilation->getAllDiagnostics());
            (void)driver->runAnalysis(*compilation);
        }
    }

    if (compilation && request.design != 0) {
        response.has_design = true;
        walkDesign(*compilation, driver->sourceManager, request.design == 2, response.design);
    }

    auto& sourceManager = driver->sourceManager;
    for (auto buffer : sourceManager.getAllBuffers()) {
        if (sourceManager.getFullPath(buffer).empty())
            continue;
        auto name = sourceManager.getRawFileName(buffer);
        if (!name.empty())
            response.deps.push_back(str(name));
    }

    // Rejected options never reach the engine, so their reasons are rendered
    // here, after the captured text, as the driver would print them.
    std::string rejected;
    for (auto& d : unsupported)
        rejected += "error: " + std::string(d.message.data(), d.message.size()) + "\n";
    auto fromPrinter = printedDiags(printed);
    for (auto& d : unsupported)
        fromPrinter.push_back(std::move(d));
    bool fatal = false;
    for (auto& d : fromPrinter) {
        fatal |= d.severity == FATAL;
        response.diagnostics.push_back(std::move(d));
    }
    for (auto& d : recorded)
        response.diagnostics.push_back(std::move(d));
    response.ok = optionsOk && !fatal && driver->diagEngine.getNumErrors() == 0;
    response.rendered = str(printed + text->getString() + rejected);
    return response;
}

void trigger_library_assert() {
    SVInt value(8, 0, false);
    (void)value.slice(0, 7);
}

} // namespace slang_sys
