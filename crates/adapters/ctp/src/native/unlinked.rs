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

//! Stub backend for builds without the `ctp-native` feature.

use crate::md::{MdBackend, MdEvent};

const UNAVAILABLE: &str = "CTP market data requires nautilus-ctp built with the `ctp-native` feature, \
    `CTP_SDK_DIR` pointing at the CTP headers, and `CTP_LIB_DIR` pointing at the CTP market data library";

/// Placeholder for the native backend. Creating one always fails.
#[derive(Debug)]
pub struct NativeMdApi {
    _private: (),
}

impl NativeMdApi {
    /// Always fails because the SDK is not linked.
    ///
    /// # Errors
    ///
    /// Returns an error if the SDK is not linked, which is always the case without the
    /// `ctp-native` feature.
    pub fn create(
        _flow_path: &str,
        _production_mode: bool,
        _events: tokio::sync::mpsc::UnboundedSender<MdEvent>,
    ) -> anyhow::Result<Self> {
        anyhow::bail!(UNAVAILABLE)
    }
}

impl MdBackend for NativeMdApi {
    fn register_front(&mut self, _address: &str) -> anyhow::Result<()> {
        anyhow::bail!(UNAVAILABLE)
    }

    fn init(&mut self) -> anyhow::Result<()> {
        anyhow::bail!(UNAVAILABLE)
    }

    fn login(
        &mut self,
        _broker_id: &str,
        _user_id: &str,
        _password: &str,
        _request_id: i32,
    ) -> anyhow::Result<()> {
        anyhow::bail!(UNAVAILABLE)
    }

    fn logout(&mut self, _broker_id: &str, _user_id: &str, _request_id: i32) -> anyhow::Result<()> {
        anyhow::bail!(UNAVAILABLE)
    }

    fn subscribe(&mut self, _instrument_ids: &[String]) -> anyhow::Result<()> {
        anyhow::bail!(UNAVAILABLE)
    }

    fn unsubscribe(&mut self, _instrument_ids: &[String]) -> anyhow::Result<()> {
        anyhow::bail!(UNAVAILABLE)
    }
}
