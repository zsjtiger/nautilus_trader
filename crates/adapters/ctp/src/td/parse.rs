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

//! Pure mapping from CTP order states to lifecycle changes.

use super::{OrderRecord, status, submit_status};

/// A lifecycle change for one order, derived from an order return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderChange {
    /// The exchange accepted the order into its book (or executed it immediately).
    Accepted,
    /// The order was rejected and will not trade. Carries the reason.
    Rejected(String),
    /// The order's remaining volume was canceled. It is terminal.
    Canceled,
}

/// Returns the lifecycle changes an order return implies, in the order they must be emitted.
///
/// `accepted` is whether the order has already been accepted by this session. An order that
/// reaches a queued or traded state for the first time is accepted first, so downstream events
/// never reference an order that was not accepted.
pub fn order_changes(record: &OrderRecord, accepted: bool) -> Vec<OrderChange> {
    if record.order_submit_status == submit_status::INSERT_REJECTED {
        return vec![OrderChange::Rejected(reason(record))];
    }

    match record.order_status {
        status::UNKNOWN => Vec::new(),
        status::ALL_TRADED | status::PART_TRADED_QUEUEING | status::NO_TRADE_QUEUEING => {
            if accepted {
                Vec::new()
            } else {
                vec![OrderChange::Accepted]
            }
        }
        status::NO_TRADE_NOT_QUEUEING => {
            if accepted {
                vec![OrderChange::Canceled]
            } else {
                vec![OrderChange::Rejected(reason(record))]
            }
        }
        status::PART_TRADED_NOT_QUEUEING | status::CANCELED => {
            if accepted {
                vec![OrderChange::Canceled]
            } else {
                vec![OrderChange::Accepted, OrderChange::Canceled]
            }
        }
        _ => Vec::new(),
    }
}

/// Returns whether an order status ends the order's lifecycle.
///
/// An order with a fully traded status is terminal, and so is one that is canceled or no longer
/// queued. A partly traded queued order is still live.
pub fn is_terminal(order_status: char) -> bool {
    matches!(
        order_status,
        status::ALL_TRADED
            | status::PART_TRADED_NOT_QUEUEING
            | status::NO_TRADE_NOT_QUEUEING
            | status::CANCELED
    )
}

fn reason(record: &OrderRecord) -> String {
    if record.status_msg.is_empty() {
        "order rejected by exchange".to_string()
    } else {
        record.status_msg.clone()
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn record(order_status: char, submit: char) -> OrderRecord {
        OrderRecord {
            front_id: 1,
            session_id: 2,
            order_ref: "7".to_string(),
            exchange_id: "SHFE".to_string(),
            order_sys_id: "123".to_string(),
            instrument_id: "rb2510".to_string(),
            direction: '0',
            order_status,
            order_submit_status: submit,
            volume_original: 10,
            volume_traded: 0,
            volume_remaining: 10,
            limit_price: 3100.0,
            status_msg: "no liquidity".to_string(),
            insert_date: "20251122".to_string(),
            insert_time: "09:30:00".to_string(),
            update_time: "09:30:00".to_string(),
            trading_day: "20251122".to_string(),
        }
    }

    #[rstest]
    #[case(status::NO_TRADE_QUEUEING, false, vec![OrderChange::Accepted])]
    #[case(status::PART_TRADED_QUEUEING, false, vec![OrderChange::Accepted])]
    #[case(status::ALL_TRADED, false, vec![OrderChange::Accepted])]
    #[case(status::NO_TRADE_QUEUEING, true, vec![])]
    #[case(status::UNKNOWN, false, vec![])]
    #[case(status::CANCELED, true, vec![OrderChange::Canceled])]
    #[case(status::CANCELED, false, vec![OrderChange::Accepted, OrderChange::Canceled])]
    #[case(status::NO_TRADE_NOT_QUEUEING, false, vec![OrderChange::Rejected("no liquidity".to_string())])]
    #[case(status::NO_TRADE_NOT_QUEUEING, true, vec![OrderChange::Canceled])]
    fn test_order_changes(
        #[case] order_status: char,
        #[case] accepted: bool,
        #[case] expected: Vec<OrderChange>,
    ) {
        let changes = order_changes(&record(order_status, submit_status::ACCEPTED), accepted);
        assert_eq!(changes, expected);
    }

    #[rstest]
    fn test_insert_rejection_takes_precedence_over_status() {
        let changes = order_changes(
            &record(status::UNKNOWN, submit_status::INSERT_REJECTED),
            false,
        );
        assert_eq!(
            changes,
            vec![OrderChange::Rejected("no liquidity".to_string())]
        );
    }

    #[rstest]
    fn test_rejection_without_message_uses_default_reason() {
        let mut rec = record(status::NO_TRADE_NOT_QUEUEING, submit_status::ACCEPTED);
        rec.status_msg.clear();
        assert_eq!(
            order_changes(&rec, false),
            vec![OrderChange::Rejected(
                "order rejected by exchange".to_string()
            )]
        );
    }

    #[rstest]
    #[case(status::ALL_TRADED, true)]
    #[case(status::CANCELED, true)]
    #[case(status::NO_TRADE_NOT_QUEUEING, true)]
    #[case(status::PART_TRADED_NOT_QUEUEING, true)]
    #[case(status::PART_TRADED_QUEUEING, false)]
    #[case(status::NO_TRADE_QUEUEING, false)]
    #[case(status::UNKNOWN, false)]
    fn test_is_terminal(#[case] order_status: char, #[case] expected: bool) {
        assert_eq!(is_terminal(order_status), expected);
    }
}
