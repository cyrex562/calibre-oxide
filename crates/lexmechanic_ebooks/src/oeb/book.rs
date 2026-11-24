use crate::oeb::container::Container;
use crate::oeb::guide::Guide;
use crate::oeb::manifest::Manifest;
use crate::oeb::metadata::Metadata;
use crate::oeb::spine::Spine;
use crate::oeb::toc::TOC;

#[derive(Debug, Clone)]
pub struct Page {
    pub name: String,
    pub href: String,
    pub type_: String,
}

#[derive(Debug, Clone, Default)]
pub struct PageList {
    pub pages: Vec<Page>,
}

impl PageList {
    pub fn new() -> Self {
        PageList { pages: Vec::new() }
    }

    pub fn add(&mut self, name: &str, href: &str, type_: &str) {
        self.pages.push(Page {
            name: name.to_string(),
            href: href.to_string(),
            type_: type_.to_string(),
        });
    }
}

pub struct OEBBook {
    pub metadata: Metadata,
    pub manifest: Manifest,
    pub spine: Spine,
    pub guide: Guide,
    pub toc: TOC,
    pub pages: PageList,
    pub container: Box<dyn Container>,
    pub version: String,
    pub uid: Option<String>,
}

impl OEBBook {
    pub fn new(container: Box<dyn Container>) -> Self {
        OEBBook {
            metadata: Metadata::new(),
            manifest: Manifest::new(),
            spine: Spine::new(),
            guide: Guide::new(),
            toc: TOC::new(),
            pages: PageList::new(),
            container,
            version: "2.0".to_string(),
            uid: None,
        }
    }
}
