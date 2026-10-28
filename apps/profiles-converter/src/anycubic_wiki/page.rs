/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The tables of a saved wiki page, each under the heading above it.
//!
//! The page is the HTML the wiki serves, saved as it is. Only its
//! headings and tables matter: every table sits below the heading that
//! names the machine it calibrates.

use scraper::{ElementRef, Html, Selector};

/// One table of the page and the heading it sits under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    /// The nearest heading above the table, or empty when none is.
    pub heading: String,
    /// The table's rows, each as the text of its cells.
    pub rows: Vec<Vec<String>>,
}

/// Every table of `html`, in the order the page shows them.
pub fn tables_of(html: &str) -> Vec<Table> {
    let page = Html::parse_document(html);
    let blocks = Selector::parse("h1, h2, h3, h4, table").expect("a constant selector");
    let rows = Selector::parse("tr").expect("a constant selector");
    let cells = Selector::parse("th, td").expect("a constant selector");

    let mut heading = String::new();
    let mut tables = Vec::new();
    for block in page.select(&blocks) {
        if block.value().name() != "table" {
            heading = text_of(block);
            continue;
        }
        tables.push(Table {
            heading: heading.clone(),
            rows: block
                .select(&rows)
                .map(|row| row.select(&cells).map(text_of).collect())
                .collect(),
        });
    }
    tables
}

/// The text of an element on one line, without the anchor mark the wiki
/// puts before each heading.
fn text_of(element: ElementRef<'_>) -> String {
    element
        .text()
        .collect::<String>()
        .replace('¶', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod page_tests {
    use super::*;

    #[test]
    fn a_table_carries_the_heading_above_it_and_its_cells_as_text() {
        let html = "<h2><a>¶</a>&nbsp;Printing Settings for&nbsp;Photon S</h2>\
            <table><tr><th>Resin Type</th><th>Standard Resin</th></tr>\
            <tr><td>Exposure Time</td><td> 8s </td></tr></table>";

        assert_eq!(
            tables_of(html),
            vec![Table {
                heading: "Printing Settings for Photon S".to_owned(),
                rows: vec![
                    vec!["Resin Type".to_owned(), "Standard Resin".to_owned()],
                    vec!["Exposure Time".to_owned(), "8s".to_owned()],
                ],
            }],
        );
    }

    #[test]
    fn each_table_takes_the_nearest_heading() {
        let html = "<h2>A</h2><table><tr><td>1</td></tr></table>\
            <h2>B</h2><p>text</p><table><tr><td>2</td></tr></table>";

        let headings: Vec<String> = tables_of(html).into_iter().map(|t| t.heading).collect();
        assert_eq!(headings, vec!["A", "B"]);
    }
}
