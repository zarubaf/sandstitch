// SPDX-License-Identifier: Apache-2.0

// The design walker: fills the bridge's design tables (`FfiDesign` in
// `src/lib.rs`) from an elaborated compilation. See
// doc/design/slang-in-process.md, "What the shim walks" and "Types".

#pragma once

namespace slang {
class SourceManager;
}
namespace slang::ast {
class Compilation;
}

namespace slang_sys {

struct FfiDesign;

/// Walks the top-level instances of `compilation` into `out`, which must be
/// empty. `signals` asks for the nets and variables of each body.
void walkDesign(slang::ast::Compilation& compilation, const slang::SourceManager& sourceManager,
                bool signals, FfiDesign& out);

} // namespace slang_sys
