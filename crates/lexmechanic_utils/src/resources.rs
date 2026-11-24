use std::path::{Path, PathBuf};
use std::env;
use lazy_static::lazy_static;
use crate::constants::config_dir;

lazy_static! {
    static ref RESOLVER: PathResolver = PathResolver::new();
}

struct PathResolver {
    locations: Vec<PathBuf>,
    user_path: PathBuf,
}

impl PathResolver {
    fn new() -> Self {
        let mut locations = Vec::new();
        
        if let Ok(path) = env::var("CALIBRE_RESOURCES_PATH") {
            let p = PathBuf::from(path);
            if p.exists() {
                locations.push(p);
            }
        }

        let user_path = config_dir().join("resources");
        if user_path.exists() {
            locations.insert(0, user_path.clone());
        }
        
        PathResolver {
            locations,
            user_path,
        }
    }
    
    fn resolve(&self, path: &str, allow_user_override: bool) -> Option<PathBuf> {
        let p = Path::new(path);
        
        for base in &self.locations {
            if !allow_user_override && *base == self.user_path {
                continue;
            }
            let candidate = base.join(p);
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }
}

pub fn get_path(path: &str, allow_user_override: bool) -> Option<PathBuf> {
    RESOLVER.resolve(path, allow_user_override)
}

pub fn get_image_path(path: &str, allow_user_override: bool) -> Option<PathBuf> {
    if path.is_empty() {
        get_path("images", allow_user_override)
    } else {
        let p = format!("images/{}", path);
        get_path(&p, allow_user_override)
    }
}

pub fn get_user_path() -> PathBuf {
    RESOLVER.user_path.clone()
}
