//! Port of `page_group.py`.
//!
//! A contiguous run of page anchors that share a numbering scheme.
//! The APNX format serializes each group as
//! `(<starting_location>,<tag>,<values>)`.

use super::page_number_type::PageNumberType;

/// Errors that mirror the Python `assert` conditions. In Python they
/// panicked (uncatchable in practice); in Rust they're typed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PageGroupError {
    #[error("Custom page group requires labels")]
    CustomWithoutLabels,
    #[error("Custom label list length ({label_count}) must match page locations ({location_count})")]
    LabelCountMismatch {
        label_count: usize,
        location_count: usize,
    },
    #[error("Custom page labels must all be non-empty")]
    EmptyLabel,
    #[error("Cannot append a non-labeled location to a Custom group; use `append_labeled`")]
    AppendNonLabeledToCustom,
    #[error("Cannot append a labeled location to a non-Custom group; use `append`")]
    AppendLabeledToNonCustom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageGroup {
    page_locations: Vec<u32>,
    page_number_type: PageNumberType,
    first_value: u32,
    /// Populated only when `page_number_type == Custom`.
    page_number_labels: Vec<String>,
}

impl PageGroup {
    /// Build a group with a single starting location. Convenience for
    /// the Python `PageGroup(page_locations=int, ...)` overload.
    pub fn single(
        location: u32,
        page_number_type: PageNumberType,
        first_value: u32,
        label: Option<&str>,
    ) -> Result<Self, PageGroupError> {
        Self::new(vec![location], page_number_type, first_value, {
            match (page_number_type, label) {
                (PageNumberType::Custom, Some(l)) => Some(vec![l.to_string()]),
                (PageNumberType::Custom, None) => return Err(PageGroupError::CustomWithoutLabels),
                _ => None,
            }
        })
    }

    /// Build a group covering `page_locations` byte offsets. For a
    /// `Custom` group, `labels.len()` must equal `page_locations.len()`
    /// and every label must be non-empty. For non-Custom groups,
    /// `labels` must be `None`.
    pub fn new(
        page_locations: Vec<u32>,
        page_number_type: PageNumberType,
        first_value: u32,
        labels: Option<Vec<String>>,
    ) -> Result<Self, PageGroupError> {
        let page_number_labels = match (page_number_type, labels) {
            (PageNumberType::Custom, None) => return Err(PageGroupError::CustomWithoutLabels),
            (PageNumberType::Custom, Some(labels)) => {
                if labels.len() != page_locations.len() {
                    return Err(PageGroupError::LabelCountMismatch {
                        label_count: labels.len(),
                        location_count: page_locations.len(),
                    });
                }
                if labels.iter().any(|l| l.is_empty()) {
                    return Err(PageGroupError::EmptyLabel);
                }
                labels
            }
            // Non-Custom groups silently drop any labels — matches the
            // Python constructor which only reads labels when
            // page_number_type == Custom.
            (_, _) => Vec::new(),
        };

        Ok(PageGroup {
            page_locations,
            page_number_type,
            first_value,
            page_number_labels,
        })
    }

    pub fn page_number_type(&self) -> PageNumberType {
        self.page_number_type
    }

    pub fn number_of_pages(&self) -> usize {
        self.page_locations.len()
    }

    pub fn last_value(&self) -> u32 {
        // Python: first_value + len(page_locations) - 1
        self.first_value + self.page_locations.len() as u32 - 1
    }

    pub fn page_locations(&self) -> &[u32] {
        &self.page_locations
    }

    /// Append a location to a non-Custom group. Errors on Custom.
    pub fn append(&mut self, location: u32) -> Result<(), PageGroupError> {
        if matches!(self.page_number_type, PageNumberType::Custom) {
            return Err(PageGroupError::AppendNonLabeledToCustom);
        }
        self.page_locations.push(location);
        Ok(())
    }

    /// Append a labeled location to a Custom group. Errors on non-Custom.
    pub fn append_labeled(&mut self, location: u32, label: &str) -> Result<(), PageGroupError> {
        if !matches!(self.page_number_type, PageNumberType::Custom) {
            return Err(PageGroupError::AppendLabeledToNonCustom);
        }
        if label.is_empty() {
            return Err(PageGroupError::EmptyLabel);
        }
        self.page_locations.push(location);
        self.page_number_labels.push(label.to_string());
        Ok(())
    }

    /// Serialize this group into the APNX page-map string form
    /// `(<starting_location>,<tag>,<values>)`. `values` is the
    /// `first_value` for Arabic/Roman groups; for Custom it's the
    /// pipe-joined labels.
    pub fn get_page_map(&self, starting_location: u32) -> String {
        let values = match self.page_number_type {
            PageNumberType::Custom => self.page_number_labels.join("|"),
            _ => self.first_value.to_string(),
        };
        format!(
            "({},{},{})",
            starting_location,
            self.page_number_type.tag(),
            values
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arabic_get_page_map_matches_python_format() {
        let g = PageGroup::new(vec![100, 200, 300], PageNumberType::Arabic, 1, None).unwrap();
        assert_eq!(g.get_page_map(1), "(1,a,1)");
        assert_eq!(g.number_of_pages(), 3);
        assert_eq!(g.last_value(), 3);
    }

    #[test]
    fn roman_get_page_map() {
        let g = PageGroup::new(vec![50], PageNumberType::Roman, 1, None).unwrap();
        assert_eq!(g.get_page_map(1), "(1,r,1)");
    }

    #[test]
    fn custom_get_page_map_joins_labels_with_pipes() {
        let g = PageGroup::new(
            vec![10, 20, 30],
            PageNumberType::Custom,
            1,
            Some(vec!["i".into(), "ii".into(), "iii".into()]),
        )
        .unwrap();
        assert_eq!(g.get_page_map(1), "(1,c,i|ii|iii)");
    }

    #[test]
    fn custom_requires_labels() {
        let err = PageGroup::new(vec![10], PageNumberType::Custom, 1, None).unwrap_err();
        assert_eq!(err, PageGroupError::CustomWithoutLabels);
    }

    #[test]
    fn custom_labels_must_match_locations_count() {
        let err = PageGroup::new(
            vec![10, 20],
            PageNumberType::Custom,
            1,
            Some(vec!["only-one".into()]),
        )
        .unwrap_err();
        assert!(matches!(err, PageGroupError::LabelCountMismatch { .. }));
    }

    #[test]
    fn custom_labels_must_be_nonempty() {
        let err = PageGroup::new(
            vec![10],
            PageNumberType::Custom,
            1,
            Some(vec![String::new()]),
        )
        .unwrap_err();
        assert_eq!(err, PageGroupError::EmptyLabel);
    }

    #[test]
    fn append_extends_arabic_group() {
        let mut g = PageGroup::new(vec![100], PageNumberType::Arabic, 1, None).unwrap();
        g.append(200).unwrap();
        assert_eq!(g.page_locations(), &[100, 200]);
    }

    #[test]
    fn append_rejects_custom_group() {
        let mut g = PageGroup::new(
            vec![10],
            PageNumberType::Custom,
            1,
            Some(vec!["i".into()]),
        )
        .unwrap();
        let err = g.append(20).unwrap_err();
        assert_eq!(err, PageGroupError::AppendNonLabeledToCustom);
    }

    #[test]
    fn append_labeled_extends_custom_group() {
        let mut g = PageGroup::new(
            vec![10],
            PageNumberType::Custom,
            1,
            Some(vec!["i".into()]),
        )
        .unwrap();
        g.append_labeled(20, "ii").unwrap();
        assert_eq!(g.number_of_pages(), 2);
        assert_eq!(g.get_page_map(1), "(1,c,i|ii)");
    }

    #[test]
    fn append_labeled_rejects_non_custom_group() {
        let mut g = PageGroup::new(vec![10], PageNumberType::Arabic, 1, None).unwrap();
        let err = g.append_labeled(20, "ii").unwrap_err();
        assert_eq!(err, PageGroupError::AppendLabeledToNonCustom);
    }

    #[test]
    fn append_labeled_rejects_empty_label() {
        let mut g = PageGroup::new(
            vec![10],
            PageNumberType::Custom,
            1,
            Some(vec!["i".into()]),
        )
        .unwrap();
        let err = g.append_labeled(20, "").unwrap_err();
        assert_eq!(err, PageGroupError::EmptyLabel);
    }
}
