// SPDX-License-Identifier: Apache-2.0

// The C++ side of the slang-sys bridge (`src/lib.rs`).

#pragma once

#include "rust/cxx.h"

namespace slang_sys {

struct FfiRequest;
struct FfiResponse;

/// `major.minor.patch` of the linked Slang library.
rust::String library_version();

/// One Slang run; see `run` in `src/lib.rs`. Throws on internal errors.
FfiResponse run(const FfiRequest& request);

/// Fails `SLANG_ASSERT(msb >= lsb)` in `SVInt::slice` (SVInt.cpp).
void trigger_library_assert();

} // namespace slang_sys
