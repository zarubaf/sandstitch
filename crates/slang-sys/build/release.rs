// SPDX-License-Identifier: Apache-2.0

// The vendored Slang release number, the one place it is written. Included
// by `src/lib.rs` and by `build.rs`, which records it for a prebuilt library.
macro_rules! release {
    () => {
        "12.0"
    };
}
