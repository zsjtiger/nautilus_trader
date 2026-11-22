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

//! Native market data backend.
//!
//! With the `ctp-native` feature the backend links the CTP-compatible SDK. Without it,
//! [`NativeMdApi`] is a stub that fails when a session starts, so the client type stays
//! available in every build.

#[cfg(feature = "ctp-native")]
mod linked;
#[cfg(not(feature = "ctp-native"))]
mod unlinked;

#[cfg(feature = "ctp-native")]
pub use linked::NativeMdApi;
#[cfg(not(feature = "ctp-native"))]
pub use unlinked::NativeMdApi;
