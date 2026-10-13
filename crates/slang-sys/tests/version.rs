// SPDX-License-Identifier: Apache-2.0

//! The linked library is the vendored release.

use slang_sys::{SLANG_VERSION, library_version};

#[test]
fn library_is_the_vendored_release() {
    let version = library_version();
    assert_eq!(
        version.rsplit_once('.').unwrap().0,
        SLANG_VERSION,
        "{version}"
    );
}
