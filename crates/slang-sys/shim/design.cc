// SPDX-License-Identifier: Apache-2.0

// Fills the design tables from Slang's elaborated symbols. Types are walked
// as written, not canonicalized, so that the alias naming a struct, union
// or enum survives. See doc/design/slang-in-process.md, "What the shim
// walks" and "Types".

#include "slang-sys/shim/design.h"

#include <map>
#include <optional>
#include <string>
#include <string_view>
#include <unordered_map>
#include <utility>

#include "slang-sys/shim/shim.h"
#include "slang-sys/src/lib.rs.h"
#include "slang/ast/Compilation.h"
#include "slang/ast/symbols/BlockSymbols.h"
#include "slang/ast/symbols/CompilationUnitSymbols.h"
#include "slang/ast/symbols/InstanceSymbols.h"
#include "slang/ast/symbols/MemberSymbols.h"
#include "slang/ast/symbols/ParameterSymbols.h"
#include "slang/ast/symbols/PortSymbols.h"
#include "slang/ast/symbols/VariableSymbols.h"
#include "slang/ast/types/AllTypes.h"
#include "slang/ast/types/TypePrinter.h"
#include "slang/numeric/SVInt.h"
#include "slang/text/SourceManager.h"

namespace slang_sys {

namespace {

using namespace slang;
using namespace slang::ast;

// `FfiType::kind`.
constexpr uint8_t LOGIC = 0;
constexpr uint8_t STRUCT = 1;
constexpr uint8_t UNION = 2;
constexpr uint8_t ENUM = 3;
constexpr uint8_t UNSUPPORTED = 4;

// `FfiPort::kind`.
constexpr uint8_t DATA_PORT = 0;
constexpr uint8_t INTERFACE_PORT = 1;
constexpr uint8_t UNSUPPORTED_PORT = 2;

// `FfiParameter::kind`.
constexpr uint8_t VALUE_PARAMETER = 0;
constexpr uint8_t TYPE_PARAMETER = 1;

// `FfiPackageParameter::kind`.
constexpr uint8_t PACKAGE_VALUE = 0;
constexpr uint8_t PACKAGE_ALIAS = 1;

constexpr std::string_view TYPE_ERROR = "only integral (bit-vector) types are supported";
constexpr std::string_view XZ_ERROR = "enum values with X or Z bits are not supported";
constexpr std::string_view REF_ERROR = "`ref` ports are not supported";
constexpr std::string_view MULTI_PORT_ERROR =
    "ports that connect several signals are not supported";

/// The prefix Slang's type printer writes for a type or alias declared in
/// `scope` (`TypePrinter.cpp`, `getLexicalPath`): nothing in a compilation
/// unit, `pkg::` in a package, `top.` in a module.
std::string scopePrefix(const Scope* scope) {
    if (!scope || scope->asSymbol().kind == SymbolKind::CompilationUnit)
        return "";
    auto& symbol = scope->asSymbol();
    std::string prefix = symbol.getLexicalPath();
    if (symbol.kind == SymbolKind::Package || symbol.kind == SymbolKind::ClassType ||
        symbol.kind == SymbolKind::CovergroupType) {
        prefix += "::";
    }
    else {
        prefix += '.';
    }
    return prefix;
}

/// Whether `alias` is the alias a type parameter introduces, which names
/// nothing: its target is named as if written in its place.
bool isTypeParameterAlias(const TypeAliasType& alias) {
    auto scope = alias.getParentScope();
    if (!scope)
        return false;
    auto symbol = scope->find(alias.name);
    return symbol && symbol->kind == SymbolKind::TypeParameter &&
           &symbol->as<TypeParameterSymbol>().getTypeAlias() == &alias;
}

/// Whether Slang's type printer can print `type`: it asserts that every
/// member of a struct or union is a variable, which fails for a struct that
/// declares an anonymous enum (its values are members of the struct's scope).
bool printable(const Type& type);

bool printableMembers(const Scope& scope) {
    for (auto& member : scope.members()) {
        if (member.kind != SymbolKind::Field || !printable(member.as<FieldSymbol>().getType()))
            return false;
    }
    return true;
}

bool printable(const Type& type) {
    switch (type.kind) {
        case SymbolKind::TypeAlias:
            return printable(type.as<TypeAliasType>().targetType.getType());
        case SymbolKind::PackedArrayType:
            return printable(type.as<PackedArrayType>().elementType);
        case SymbolKind::FixedSizeUnpackedArrayType:
            return printable(type.as<FixedSizeUnpackedArrayType>().elementType);
        case SymbolKind::PackedStructType:
            return printableMembers(type.as<PackedStructType>());
        case SymbolKind::PackedUnionType:
            return printableMembers(type.as<PackedUnionType>());
        case SymbolKind::UnpackedStructType:
            return printableMembers(type.as<UnpackedStructType>());
        case SymbolKind::UnpackedUnionType:
            return printableMembers(type.as<UnpackedUnionType>());
        default:
            return true;
    }
}

std::string rangeText(const ConstantRange& range) {
    return "[" + std::to_string(range.left) + ":" + std::to_string(range.right) + "]";
}

/// Slang's display of `type` (`Type::toString`; a struct or union its printer
/// cannot print is shown as `struct{...}` or `union{...}`), with the aliases of type
/// parameters seen through: `real` for a port of type `T = real`, also
/// inside arrays (`real$[0:1]`). Other aliases print as their names.
std::string typeText(const Type& type) {
    if (type.kind == SymbolKind::TypeAlias && isTypeParameterAlias(type.as<TypeAliasType>()))
        return typeText(type.as<TypeAliasType>().targetType.getType());
    if (type.kind == SymbolKind::PackedArrayType) {
        // As `TypePrinter`: the innermost element, then every packed
        // dimension down to it, through aliases.
        std::string dims;
        auto curr = &type.as<PackedArrayType>();
        while (true) {
            dims += rangeText(curr->range);
            if (!curr->elementType.isPackedArray())
                break;
            curr = &curr->elementType.getCanonicalType().as<PackedArrayType>();
        }
        return typeText(curr->elementType) + dims;
    }
    if (type.kind != SymbolKind::TypeAlias && type.isUnpackedArray()) {
        // As `TypePrinter::printUnpackedArray`: the innermost element, `$`,
        // then the dimensions.
        auto element = &type;
        do {
            element = element->getArrayElementType();
        } while (element->isUnpackedArray());
        std::string text = typeText(*element) + "$";
        for (auto dim = &type; dim->isUnpackedArray();
             dim = &dim->getArrayElementType()->getCanonicalType()) {
            switch (dim->kind) {
                case SymbolKind::FixedSizeUnpackedArrayType:
                    text += rangeText(dim->as<FixedSizeUnpackedArrayType>().range);
                    break;
                case SymbolKind::AssociativeArrayType: {
                    auto index = dim->as<AssociativeArrayType>().indexType;
                    text += index ? "[" + typeText(*index) + "]" : "[*]";
                    break;
                }
                case SymbolKind::QueueType: {
                    auto bound = dim->as<QueueType>().maxBound;
                    text += bound ? "[$:" + std::to_string(bound) + "]" : "[$]";
                    break;
                }
                default: // dynamic and DPI open arrays
                    text += "[]";
                    break;
            }
        }
        return text;
    }
    if (!printable(type)) {
        // Slang's printer would assert on the members: name the type.
        if (type.kind == SymbolKind::TypeAlias)
            return scopePrefix(type.getParentScope()) + std::string(type.name);
        auto& canonical = type.getCanonicalType();
        bool isUnion = canonical.kind == SymbolKind::PackedUnionType ||
                       canonical.kind == SymbolKind::UnpackedUnionType;
        return isUnion ? "union{...}" : "struct{...}";
    }
    return type.toString();
}

FfiLoc location(const SourceManager& sourceManager, const Symbol& symbol) {
    FfiLoc loc{};
    if (!symbol.location || symbol.location == SourceLocation::NoLocation)
        return loc;
    loc.file = str(sourceManager.getFileName(symbol.location));
    loc.line = uint32_t(sourceManager.getLineNumber(symbol.location));
    loc.column = uint32_t(sourceManager.getColumnNumber(symbol.location));
    return loc;
}

class Walker {
public:
    Walker(Compilation& compilation, const SourceManager& sourceManager, bool signals,
           FfiDesign& out) :
        compilation(compilation), sourceManager(sourceManager), signals(signals), out(out) {}

    void top(const InstanceSymbol& instance) {
        if (instance.body.flags.has(InstanceFlags::Uninstantiated))
            return;
        FfiTop top{};
        top.name = str(instance.name);
        top.body = bodyOf(instance);
        out.tops.push_back(std::move(top));
    }

    /// Adds the packages of the compilation units: their value parameters
    /// and type aliases.
    void packages() {
        for (auto unit : compilation.getRoot().compilationUnits) {
            for (auto& member : unit->members()) {
                if (member.kind == SymbolKind::Package)
                    out.packages.push_back(package(member.as<PackageSymbol>()));
            }
        }
    }

private:
    FfiPackage package(const PackageSymbol& symbol) {
        FfiPackage package{};
        package.name = str(symbol.name);
        for (auto& member : symbol.members()) {
            FfiPackageParameter parameter{};
            parameter.name = str(member.name);
            if (member.kind == SymbolKind::Parameter) {
                // The constant as `toString` prints it with unknown bits exact.
                parameter.kind = PACKAGE_VALUE;
                parameter.value = str(member.as<ParameterSymbol>().getValue().toString(
                    SVInt::MAX_BITS, /* exactUnknowns */ true, /* useAssignmentPatterns */ true));
            }
            else if (member.kind == SymbolKind::TypeAlias) {
                // The alias as Slang's type printer expands it, named by its
                // package.
                auto& alias = member.as<TypeAliasType>();
                parameter.kind = PACKAGE_ALIAS;
                if (printable(alias)) {
                    TypePrinter printer;
                    printer.append(alias);
                    parameter.value = str(printer.toString());
                }
                else {
                    // Only the name: the printer would assert on the members.
                    parameter.value = str(scopePrefix(alias.getParentScope()) +
                                          std::string(alias.name));
                }
                parameter.ty = intern(alias, "");
            }
            else {
                continue;
            }
            package.parameters.push_back(std::move(parameter));
        }
        return package;
    }

    /// The index of the body of `instance`: the canonical body it shares
    /// with identical instances when Slang cached it, else its own. Each
    /// body is walked once, children first, so that a body is numbered after
    /// every body it refers to.
    uint32_t bodyOf(const InstanceSymbol& instance) {
        auto canonical = instance.getCanonicalBody();
        auto& symbol = canonical && !compilation.hasFlag(CompilationFlags::DisableInstanceCaching)
                           ? *canonical
                           : instance.body;
        if (auto it = bodies.find(&symbol); it != bodies.end())
            return it->second;
        FfiBody body{};
        for (auto member : symbol.getPortList()) {
            switch (member->kind) {
                case SymbolKind::Port:
                    body.ports.push_back(port(member->as<PortSymbol>()));
                    break;
                case SymbolKind::MultiPort:
                    body.ports.push_back(multiPort(member->as<MultiPortSymbol>()));
                    break;
                case SymbolKind::InterfacePort:
                    body.ports.push_back(interfacePort(member->as<InterfacePortSymbol>()));
                    break;
                default:
                    break;
            }
        }
        for (auto& member : symbol.members()) {
            if (member.kind == SymbolKind::Parameter)
                body.parameters.push_back(valueParameter(member.as<ParameterSymbol>()));
            else if (member.kind == SymbolKind::TypeParameter)
                body.parameters.push_back(typeParameter(member.as<TypeParameterSymbol>()));
        }
        if (signals)
            addSignals(symbol, body);
        addChildren(symbol, "", body);
        auto index = uint32_t(out.bodies.size());
        out.bodies.push_back(std::move(body));
        bodies.emplace(&symbol, index);
        return index;
    }

    /// Adds the nets and variables declared directly in `symbol` to `body`;
    /// a port appears as its internal net or variable.
    void addSignals(const InstanceBodySymbol& symbol, FfiBody& body) {
        for (auto& member : symbol.members()) {
            if (member.kind != SymbolKind::Net && member.kind != SymbolKind::Variable)
                continue;
            auto& type = member.as<ValueSymbol>().getType();
            FfiSignal signal{};
            signal.name = str(member.name);
            signal.type_text = str(typeText(type));
            signal.width = type.getBitstreamWidth();
            body.signals.push_back(std::move(signal));
        }
    }

    static std::string join(const std::string& prefix, const std::string& name) {
        return prefix.empty() ? name : prefix + "." + name;
    }

    /// Adds the instances of `scope` to `body`: its own and those of its
    /// instantiated generate blocks, which `prefix` names.
    void addChildren(const Scope& scope, const std::string& prefix, FfiBody& body) {
        for (auto& member : scope.members()) {
            switch (member.kind) {
                case SymbolKind::Instance:
                    addChild(member.as<InstanceSymbol>(), std::string(member.name), prefix, body);
                    break;
                case SymbolKind::InstanceArray:
                    addArray(member.as<InstanceArraySymbol>(), std::string(member.name), prefix,
                             body);
                    break;
                case SymbolKind::UninstantiatedDef: {
                    auto& def = member.as<UninstantiatedDefSymbol>();
                    FfiChild child{};
                    child.name = str(def.name);
                    child.definition = str(def.definitionName);
                    child.prefix = str(prefix);
                    body.children.push_back(std::move(child));
                    break;
                }
                case SymbolKind::GenerateBlock: {
                    auto& block = member.as<GenerateBlockSymbol>();
                    if (!block.isUninstantiated)
                        addChildren(block, join(prefix, block.getExternalName()), body);
                    break;
                }
                case SymbolKind::GenerateBlockArray: {
                    auto& array = member.as<GenerateBlockArraySymbol>();
                    if (!array.valid)
                        break;
                    auto name = array.getExternalName();
                    for (auto entry : array.entries) {
                        if (entry->isUninstantiated)
                            continue;
                        auto index = entry->getArrayIndex();
                        auto element = index ? name + "[" + index->toString() + "]" : name;
                        addChildren(*entry, join(prefix, element), body);
                    }
                    break;
                }
                default:
                    break;
            }
        }
    }

    /// Adds each element of `array` as a child named `name` and its indices
    /// as declared, in declaration order (the left bound first). Element `k`
    /// of the array has index `range.lower() + k`, whichever way the range
    /// runs.
    void addArray(const InstanceArraySymbol& array, const std::string& name,
                  const std::string& prefix, FfiBody& body) {
        size_t count = array.elements.size();
        bool descending = array.range.left >= array.range.right;
        for (size_t i = 0; i < count; i++) {
            size_t k = descending ? count - 1 - i : i;
            auto element = name + "[" + std::to_string(array.range.lower() + int(k)) + "]";
            auto symbol = array.elements[k];
            if (symbol->kind == SymbolKind::InstanceArray)
                addArray(symbol->as<InstanceArraySymbol>(), element, prefix, body);
            else if (symbol->kind == SymbolKind::Instance)
                addChild(symbol->as<InstanceSymbol>(), element, prefix, body);
        }
    }

    void addChild(const InstanceSymbol& instance, const std::string& name,
                  const std::string& prefix, FfiBody& body) {
        if (instance.body.flags.has(InstanceFlags::Uninstantiated))
            return;
        FfiChild child{};
        child.name = str(name);
        child.definition = str(instance.getDefinition().name);
        child.prefix = str(prefix);
        child.has_body = true;
        child.body = bodyOf(instance);
        body.children.push_back(std::move(child));
    }

    FfiParameter valueParameter(const ParameterSymbol& symbol) {
        FfiParameter parameter{};
        parameter.name = str(symbol.name);
        parameter.kind = VALUE_PARAMETER;
        parameter.is_local = symbol.isLocalParam();
        parameter.ty = intern(symbol.getType(), "");
        return parameter;
    }

    FfiParameter typeParameter(const TypeParameterSymbol& symbol) {
        FfiParameter parameter{};
        parameter.name = str(symbol.name);
        parameter.kind = TYPE_PARAMETER;
        parameter.is_local = symbol.isLocalParam();
        return parameter;
    }

    FfiPort port(const PortSymbol& symbol) {
        FfiPort port{};
        port.name = str(symbol.name);
        port.loc = location(sourceManager, symbol);
        auto& type = symbol.getType();
        switch (symbol.direction) {
            case ArgumentDirection::In:
                port.dir = 0;
                break;
            case ArgumentDirection::Out:
                port.dir = 1;
                break;
            case ArgumentDirection::InOut:
                port.dir = 2;
                break;
            case ArgumentDirection::Ref:
                port.kind = UNSUPPORTED_PORT;
                port.type_text = str(typeText(type));
                port.error = str(REF_ERROR);
                return port;
        }
        port.kind = DATA_PORT;
        port.ty = intern(type, "");
        return port;
    }

    FfiPort multiPort(const MultiPortSymbol& symbol) {
        FfiPort port{};
        port.name = str(symbol.name);
        port.loc = location(sourceManager, symbol);
        port.kind = UNSUPPORTED_PORT;
        port.type_text = str(typeText(symbol.getType()));
        port.error = str(MULTI_PORT_ERROR);
        return port;
    }

    FfiPort interfacePort(const InterfacePortSymbol& symbol) {
        FfiPort port{};
        port.name = str(symbol.name);
        port.loc = location(sourceManager, symbol);
        port.kind = INTERFACE_PORT;
        if (symbol.interfaceDef)
            port.interface = str(symbol.interfaceDef->name);
        port.modport = str(symbol.modport);
        port.has_modport = !symbol.modport.empty();
        return port;
    }

    /// The index of the entry for `type` reached under `name`, the printed
    /// outermost alias (empty when no alias reached it). An unsupported
    /// type, or one containing an unsupported part, is a single entry of
    /// kind `UNSUPPORTED` with the display of `type` itself.
    uint32_t intern(const Type& type, const std::string& name) {
        auto key = std::make_pair(&type, name);
        if (auto it = types.find(key); it != types.end())
            return it->second;
        FfiType entry{};
        auto error = shape(type, name, entry);
        // Shaping adds the entries of the fields first.
        auto index = uint32_t(out.types.size());
        if (error) {
            entry = FfiType{};
            entry.kind = UNSUPPORTED;
            entry.type_text = str(typeText(type));
            entry.error = str(*error);
            reasons.emplace(index, *error);
        }
        out.types.push_back(std::move(entry));
        types.emplace(std::move(key), index);
        return index;
    }

    /// Fills `entry` with `type`, named `name` if it is a struct, union or
    /// enum; the reason when it is not supported.
    std::optional<std::string_view> shape(const Type& type, const std::string& name,
                                          FfiType& entry) {
        switch (type.kind) {
            case SymbolKind::TypeAlias: {
                auto& alias = type.as<TypeAliasType>();
                auto& target = alias.targetType.getType();
                if (!name.empty() || isTypeParameterAlias(alias))
                    return shape(target, name, entry);
                return shape(target, scopePrefix(alias.getParentScope()) + std::string(alias.name),
                             entry);
            }
            case SymbolKind::ScalarType:
                entry.kind = LOGIC;
                entry.is_signed = type.isSigned();
                return std::nullopt;
            case SymbolKind::PredefinedIntegerType:
                entry.kind = LOGIC;
                entry.is_signed = type.isSigned();
                entry.packed_dims.push_back(FfiRange{int32_t(type.getBitWidth()) - 1, 0});
                return std::nullopt;
            case SymbolKind::PackedArrayType: {
                // An array names neither itself nor its element: the element
                // keeps its own name.
                // Dimensions are added outermost first, before the element's.
                auto& array = type.as<PackedArrayType>();
                entry.packed_dims.push_back(FfiRange{array.range.left, array.range.right});
                return shape(array.elementType, "", entry);
            }
            case SymbolKind::FixedSizeUnpackedArrayType: {
                auto& array = type.as<FixedSizeUnpackedArrayType>();
                entry.unpacked_dims.push_back(FfiRange{array.range.left, array.range.right});
                return shape(array.elementType, "", entry);
            }
            case SymbolKind::PackedStructType: {
                auto& t = type.as<PackedStructType>();
                return aggregate(STRUCT, true, t.isSigned, t, t, name, "s$", t.systemId, entry);
            }
            case SymbolKind::PackedUnionType: {
                auto& t = type.as<PackedUnionType>();
                return aggregate(UNION, true, t.isSigned, t, t, name, "u$", t.systemId, entry);
            }
            case SymbolKind::UnpackedStructType: {
                auto& t = type.as<UnpackedStructType>();
                return aggregate(STRUCT, false, false, t, t, name, "s$", t.systemId, entry);
            }
            case SymbolKind::UnpackedUnionType: {
                auto& t = type.as<UnpackedUnionType>();
                return aggregate(UNION, false, false, t, t, name, "u$", t.systemId, entry);
            }
            case SymbolKind::EnumType:
                return enumeration(type.as<EnumType>(), name, entry);
            default:
                return TYPE_ERROR;
        }
    }

    /// `name`, or the name Slang's type printer gives the type: its scope's
    /// prefix and the enum's own name, or for an anonymous type `s$`, `u$`
    /// or `e$` and its id.
    static std::string typeName(const Type& type, const std::string& name, std::string_view tag,
                                int systemId) {
        if (!name.empty())
            return name;
        // Slang names an enum declared in a typedef by the typedef, also
        // when the typedef adds dimensions (`EnumType::fromSyntax`), and the
        // type printer prints that name.
        if (type.kind == SymbolKind::EnumType && !type.name.empty())
            return scopePrefix(type.getParentScope()) + std::string(type.name);
        return scopePrefix(type.getParentScope()) + std::string(tag) + std::to_string(systemId);
    }

    std::optional<std::string_view> aggregate(uint8_t kind, bool packed, bool isSigned,
                                              const Type& type, const Scope& members,
                                              const std::string& name,
                                              std::string_view tag, int systemId,
                                              FfiType& entry) {
        for (auto& field : members.membersOfType<FieldSymbol>()) {
            auto index = intern(field.getType(), "");
            if (auto it = reasons.find(index); it != reasons.end())
                return it->second;
            FfiField f{};
            f.name = str(field.name);
            f.ty = index;
            entry.fields.push_back(std::move(f));
        }
        entry.kind = kind;
        entry.name = str(typeName(type, name, tag, systemId));
        entry.packed = packed;
        entry.is_signed = isSigned;
        return std::nullopt;
    }

    std::optional<std::string_view> enumeration(const EnumType& type, const std::string& name,
                                                FfiType& entry) {
        auto width = type.baseType.getBitWidth();
        bool isSigned = type.baseType.isSigned();
        if (width == 0)
            return TYPE_ERROR;
        for (auto& member : type.values()) {
            auto& constant = member.getValue();
            if (!constant.isInteger())
                return TYPE_ERROR;
            auto value = constant.integer();
            if (value.hasUnknown())
                return XZ_ERROR;
            if (value.getBitWidth() < width)
                value = value.extend(width, value.isSigned());
            else if (value.getBitWidth() > width)
                value = value.trunc(width);
            FfiVariant variant{};
            variant.name = str(member.name);
            variant.width = width;
            variant.is_signed = isSigned;
            auto words = value.getRawPtr();
            for (bitwidth_t byte = 0; byte < (width + 7) / 8; byte++)
                variant.value.push_back(uint8_t(words[byte / 8] >> (8 * (byte % 8))));
            entry.variants.push_back(std::move(variant));
        }
        entry.kind = ENUM;
        entry.name = str(typeName(type, name, "e$", type.systemId));
        return std::nullopt;
    }

    Compilation& compilation;
    const SourceManager& sourceManager;
    bool signals;
    FfiDesign& out;
    std::map<std::pair<const Type*, std::string>, uint32_t> types;
    /// The reason of each unsupported entry, by index; always one of the
    /// `*_ERROR` constants.
    std::unordered_map<uint32_t, std::string_view> reasons;
    std::unordered_map<const InstanceBodySymbol*, uint32_t> bodies;
};

} // namespace

void walkDesign(Compilation& compilation, const SourceManager& sourceManager, bool signals,
                FfiDesign& out) {
    out.has_signals = signals;
    Walker walker(compilation, sourceManager, signals, out);
    for (auto instance : compilation.getRoot().topInstances)
        walker.top(*instance);
    walker.packages();
}

} // namespace slang_sys
