// SPDX-FileCopyrightText: Distronode Corporation
//
// SPDX-License-Identifier: Apache-2.0

//! `district-ai`: District AI for Linux. Everything is in the library; see
//! `lib.rs`.

#![forbid(unsafe_code)]

fn main() -> district_app::ExitCode {
    district_app::run()
}
