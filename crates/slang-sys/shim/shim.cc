// SPDX-License-Identifier: Apache-2.0

#include "slang-sys/shim/shim.h"

#include <string>

#include "slang/util/VersionInfo.h"

namespace slang_sys {

rust::String library_version() {
    using slang::VersionInfo;
    return rust::String(std::to_string(VersionInfo::getMajor()) + "." +
                        std::to_string(VersionInfo::getMinor()) + "." +
                        std::to_string(VersionInfo::getPatch()));
}

} // namespace slang_sys
