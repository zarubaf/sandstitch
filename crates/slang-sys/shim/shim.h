// SPDX-License-Identifier: Apache-2.0

// The C++ side of the slang-sys bridge (`src/lib.rs`).

#pragma once

#include <string_view>

#include "rust/cxx.h"

namespace slang_sys {

/// Every string the shim passes to Rust goes through here: invalid UTF-8 (a
/// Latin-1 byte in a string literal, say) becomes U+FFFD instead of making
/// `rust::String` throw and the whole run fail.
inline rust::String str(std::string_view text) {
    return rust::String::lossy(text.data(), text.size());
}

struct FfiRequest;
struct FfiResponse;

/// `major.minor.patch` of the linked Slang library.
rust::String library_version();

/// One Slang run; see `run` in `src/lib.rs`. Throws on internal errors.
FfiResponse run(const FfiRequest& request);

/// Fails `SLANG_ASSERT(msb >= lsb)` in `SVInt::slice` (SVInt.cpp).
void trigger_library_assert();

} // namespace slang_sys
