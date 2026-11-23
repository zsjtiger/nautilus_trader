// -------------------------------------------------------------------------------------------------
//  Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved.
//  https://nautechsystems.io
//
//  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
//  You may not use this file except in compliance with the License.
//  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
// -------------------------------------------------------------------------------------------------

//! Builds the C++ shim and links the CTP market data library when the `ctp-native` feature is on.
//!
//! - `CTP_SDK_DIR`: directory containing `ThostFtdcMdApi.h` and `ThostFtdcUserApiStruct.h`.
//! - `CTP_LIB_DIR`: directory containing `libthostmduserapi_se.so` (or the platform equivalent).
//!   The openctp wheel ships hashed file names, so point this at a directory with a symlink named
//!   `libthostmduserapi_se.so`.

use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=CTP_SDK_DIR");
    println!("cargo:rerun-if-env-changed=CTP_LIB_DIR");
    println!("cargo:rerun-if-changed=native/ctp_md_shim.h");
    println!("cargo:rerun-if-changed=native/ctp_md_shim.cpp");
    println!("cargo:rerun-if-changed=native/ctp_td_shim.h");
    println!("cargo:rerun-if-changed=native/ctp_td_shim.cpp");

    if env::var_os("CARGO_FEATURE_CTP_NATIVE").is_none() {
        return;
    }

    let sdk_dir = required_dir("CTP_SDK_DIR");
    let lib_dir = required_dir("CTP_LIB_DIR");

    // The SDK directory is passed as a system include so warnings in the vendor headers stay quiet.
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("native/ctp_md_shim.cpp")
        .file("native/ctp_td_shim.cpp")
        .flag("-isystem")
        .flag(sdk_dir.to_str().expect("CTP_SDK_DIR must be valid UTF-8"))
        .include("native")
        .compile("nautilus_ctp_md_shim");

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=thostmduserapi_se");
}

fn required_dir(name: &str) -> PathBuf {
    let value = env::var(name).unwrap_or_else(|_| {
        panic!("feature `ctp-native` requires the `{name}` environment variable to be set")
    });
    let path = PathBuf::from(value);
    assert!(
        path.is_dir(),
        "`{name}` must point to an existing directory: {}",
        path.display()
    );
    path
}
