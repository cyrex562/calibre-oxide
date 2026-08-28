//! Port of `old_src/src/calibre/ebooks/rtf2xml/table_info.py`
//! (`TableInfo`).
//!
//! Consumes [`super::table::TableSummary`] (Python's `table_data`,
//! [`super::table::make_table`]'s own second return value) and inserts
//! a `<table>` open tag -- with `number-of-columns`,
//! `number-of-rows`, `average-cells-per-row`, and `average-cell-width`
//! attributes, in that order -- right before each `mi<mk<tabl-start`
//! marker, and a matching close tag right before each
//! `mi<mk<table-end_` marker. One [`TableSummary`] is consumed per
//! `tabl-start` marker seen, in document order.
//!
//! Operates directly on intermediate-format content (see
//! [`super::process_tokens`]'s module docs) rather than reopening
//! files -- the temp-file / [`super::copy`] / rename dance around the
//! real pass is pipeline plumbing, not ported here.

use thiserror::Error;

use super::table::{Stat, TableSummary};

/// Port of the `run_level > 3`-gated `raise self.__bug_handler(msg)`
/// in `insert_info`, reached when a `tabl-start` marker is found with
/// no corresponding [`TableSummary`] left to consume -- "shouldn't
/// happen" per Python's own comment.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TableInfoError {
    #[error("Not enough data for each table\n")]
    NotEnoughTableData,
}

pub type Result<T> = std::result::Result<T, TableInfoError>;

fn stat_to_string<T: ToString>(stat: &Stat<T>) -> String {
    match stat {
        Stat::NotDefined => "not-defined".to_string(),
        Stat::Value(v) => v.to_string(),
    }
}

fn table_open_tag(summary: &TableSummary) -> String {
    format!(
        "mi<tg<open-att__<table<number-of-columns>{}<number-of-rows>{}<average-cells-per-row>{}<average-cell-width>{}\n",
        summary.number_of_columns,
        summary.number_of_rows,
        stat_to_string(&summary.average_cells_per_row),
        stat_to_string(&summary.average_cell_width),
    )
}

/// Port of `TableInfo.insert_info`, operating directly on
/// intermediate-format content (see this module's own docs) rather
/// than reopening a file. `table_data` should be
/// [`super::table::make_table`]'s own second return value, in the
/// same document order.
pub fn insert_info(content: &str, table_data: &[TableSummary], run_level: u32) -> Result<String> {
    let mut out = String::new();
    let mut remaining = table_data.iter();

    for line in content.lines() {
        if line == "mi<mk<tabl-start" {
            match remaining.next() {
                Some(summary) => out.push_str(&table_open_tag(summary)),
                None => {
                    if run_level > 3 {
                        return Err(TableInfoError::NotEnoughTableData);
                    }
                    out.push_str("mi<tg<open______<table\n");
                }
            }
        } else if line == "mi<mk<table-end_" {
            out.push_str("mi<tg<close_____<table\n");
        }
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_without_table_markers_passes_through_unchanged() {
        let content = "tx<nu<__________<hello\n";
        assert_eq!(insert_info(content, &[], 1).unwrap(), content);
    }

    #[test]
    fn inserts_an_open_and_close_tag_around_a_tables_markers() {
        let summary = TableSummary {
            number_of_columns: 2,
            number_of_rows: 3,
            average_cells_per_row: Stat::Value(2),
            average_cell_width: Stat::Value("100.00".to_string()),
        };
        let content = "mi<mk<tabl-start\ntx<nu<__________<cell\nmi<mk<table-end_\n";
        let out = insert_info(content, &[summary], 1).unwrap();
        assert!(
            out.starts_with(
                "mi<tg<open-att__<table<number-of-columns>2<number-of-rows>3<average-cells-per-row>2<average-cell-width>100.00\nmi<mk<tabl-start\n"
            ),
            "{out}"
        );
        assert!(out.contains("mi<tg<close_____<table\nmi<mk<table-end_\n"), "{out}");
    }

    #[test]
    fn not_defined_stats_render_as_the_literal_not_defined_string() {
        let summary = TableSummary::default();
        let content = "mi<mk<tabl-start\n";
        let out = insert_info(content, &[summary], 1).unwrap();
        assert!(
            out.contains("<average-cells-per-row>not-defined<average-cell-width>not-defined"),
            "{out}"
        );
    }

    #[test]
    fn multiple_tables_consume_their_summaries_in_document_order() {
        let a = TableSummary { number_of_columns: 1, ..Default::default() };
        let b = TableSummary { number_of_columns: 9, ..Default::default() };
        let content = "mi<mk<tabl-start\nmi<mk<table-end_\nmi<mk<tabl-start\nmi<mk<table-end_\n";
        let out = insert_info(content, &[a, b], 1).unwrap();
        let first = out.find("<number-of-columns>1").unwrap();
        let second = out.find("<number-of-columns>9").unwrap();
        assert!(first < second, "{out}");
    }

    #[test]
    fn exhausted_table_data_falls_back_to_a_bare_open_tag_at_low_run_level() {
        let content = "mi<mk<tabl-start\n";
        let out = insert_info(content, &[], 1).unwrap();
        assert!(out.starts_with("mi<tg<open______<table\nmi<mk<tabl-start\n"), "{out}");
    }

    #[test]
    fn exhausted_table_data_errors_at_high_run_level() {
        let content = "mi<mk<tabl-start\n";
        assert_eq!(insert_info(content, &[], 4).unwrap_err(), TableInfoError::NotEnoughTableData);
    }
}
