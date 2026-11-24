//! Port of `pages.py`.
//!
//! A sequence of `PageGroup`s representing the entire APNX page map.
//! Provides the flat page-location list and the concatenated
//! `page_maps` string that the APNX writer embeds in the file.

use super::page_group::PageGroup;
use super::page_number_type::PageNumberType;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pages {
    groups: Vec<PageGroup>,
}

impl Pages {
    pub fn new() -> Self {
        Self { groups: Vec::new() }
    }

    /// Convenience mirroring the Python
    /// `Pages(page_locations=[...])` overload — wraps the locations
    /// in a single Arabic-numbered group starting at 1.
    pub fn from_arabic_locations(locations: Vec<u32>) -> Self {
        let mut p = Self::new();
        if !locations.is_empty() {
            // Safe: new() with Arabic never fails.
            let group = PageGroup::new(locations, PageNumberType::Arabic, 1, None)
                .expect("Arabic PageGroup construction is infallible");
            p.push(group);
        }
        p
    }

    pub fn push(&mut self, group: PageGroup) {
        self.groups.push(group);
    }

    /// Panics if the collection is empty — matches the Python
    /// `__pages_groups[-1]` which raises `IndexError`. If a
    /// non-panicking accessor is needed, use `last`.
    pub fn last_group(&self) -> &PageGroup {
        self.groups.last().expect("last_group called on empty Pages")
    }

    pub fn last(&self) -> Option<&PageGroup> {
        self.groups.last()
    }

    pub fn last_mut(&mut self) -> Option<&mut PageGroup> {
        self.groups.last_mut()
    }

    pub fn groups(&self) -> &[PageGroup] {
        &self.groups
    }

    /// Concatenate every group's `get_page_map` with a comma, advancing
    /// the running starting-location by each group's `number_of_pages`.
    pub fn page_maps(&self) -> String {
        let mut location: u32 = 1;
        let mut parts: Vec<String> = Vec::with_capacity(self.groups.len());
        for group in &self.groups {
            parts.push(group.get_page_map(location));
            location += group.number_of_pages() as u32;
        }
        parts.join(",")
    }

    /// Flat list of every group's page locations, in order.
    pub fn page_locations(&self) -> Vec<u32> {
        self.groups
            .iter()
            .flat_map(|g| g.page_locations().iter().copied())
            .collect()
    }

    pub fn number_of_pages(&self) -> usize {
        self.groups.iter().map(|g| g.number_of_pages()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pages_number_of_pages_zero() {
        let p = Pages::new();
        assert_eq!(p.number_of_pages(), 0);
        assert_eq!(p.page_locations(), Vec::<u32>::new());
        assert_eq!(p.page_maps(), "");
    }

    #[test]
    fn from_arabic_locations_wraps_in_single_group() {
        let p = Pages::from_arabic_locations(vec![100, 200, 300]);
        assert_eq!(p.number_of_pages(), 3);
        assert_eq!(p.page_locations(), vec![100, 200, 300]);
        assert_eq!(p.page_maps(), "(1,a,1)");
    }

    #[test]
    fn page_maps_advances_starting_location_per_group() {
        let mut p = Pages::new();
        p.push(PageGroup::new(vec![10, 20], PageNumberType::Roman, 1, None).unwrap());
        p.push(PageGroup::new(vec![30, 40, 50], PageNumberType::Arabic, 1, None).unwrap());
        // First group has 2 pages starting at 1; next group starts at 3.
        assert_eq!(p.page_maps(), "(1,r,1),(3,a,1)");
    }

    #[test]
    fn page_locations_flattens_in_order() {
        let mut p = Pages::new();
        p.push(PageGroup::new(vec![10, 20], PageNumberType::Roman, 1, None).unwrap());
        p.push(PageGroup::new(vec![30], PageNumberType::Arabic, 1, None).unwrap());
        assert_eq!(p.page_locations(), vec![10, 20, 30]);
    }

    #[test]
    fn last_group_returns_most_recently_pushed() {
        let mut p = Pages::new();
        p.push(PageGroup::new(vec![10], PageNumberType::Roman, 1, None).unwrap());
        p.push(PageGroup::new(vec![20], PageNumberType::Arabic, 1, None).unwrap());
        assert_eq!(p.last_group().page_number_type(), PageNumberType::Arabic);
    }
}
