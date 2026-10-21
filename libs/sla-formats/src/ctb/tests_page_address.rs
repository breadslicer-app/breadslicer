/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

use proptest::prelude::*;

use super::page_address::{PAGE_BYTES, PageAddress};

#[test]
fn an_address_below_four_gibibytes_is_one_page_in() {
    assert_eq!(
        PageAddress::of(1_000),
        PageAddress {
            page: 0,
            offset: 1_000,
        },
    );
}

#[test]
fn the_last_address_of_a_page_stays_in_it() {
    assert_eq!(
        PageAddress::of(PAGE_BYTES - 1),
        PageAddress {
            page: 0,
            offset: u32::MAX,
        },
    );
}

#[test]
fn the_next_address_opens_the_next_page() {
    assert_eq!(
        PageAddress::of(PAGE_BYTES),
        PageAddress { page: 1, offset: 0 },
    );
}

proptest! {
    #[test]
    fn a_split_address_adds_back_up_to_itself(address in 0u64..(1 << 48)) {
        let split = PageAddress::of(address);

        prop_assert_eq!(
            u64::from(split.page) * PAGE_BYTES + u64::from(split.offset),
            address,
        );
    }
}
