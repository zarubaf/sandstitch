// SPDX-License-Identifier: Apache-2.0

// The C++ side of the slang-sys bridge (`src/lib.rs`).

#pragma once

#include "rust/cxx.h"

namespace slang_sys {

/// `major.minor.patch` of the linked Slang library.
rust::String library_version();

} // namespace slang_sys
