use crate::Library;
use anyhow::{anyhow, Result};
use std::collections::HashMap;

pub struct CmdSavedSearches;

impl CmdSavedSearches {
    pub fn new() -> Self {
        CmdSavedSearches
    }

    pub fn run(&self, db: &mut Library, args: &[String]) -> Result<()> {
        if args.is_empty() {
            return Err(anyhow!(
                "Usage: saved_searches list|add|remove [name] [query]"
            ));
        }

        let action = &args[0];

        let prefs_json = db
            .get_preference("saved_searches")?
            .unwrap_or_else(|| "{}".to_string());
        let mut searches: HashMap<String, String> =
            serde_json::from_str(&prefs_json).unwrap_or_default();

        match action.as_str() {
            "list" => {
                if searches.is_empty() {
                    println!("No saved searches found.");
                } else {
                    println!("{:<20} {}", "Name", "Query");
                    println!("{:<20} {}", "----", "-----");
                    for (name, query) in &searches {
                        println!("{:<20} {}", name, query);
                    }
                }
            }
            "add" => {
                if args.len() < 3 {
                    return Err(anyhow!("Usage: saved_searches add <name> <query>"));
                }
                let name = &args[1];
                let query = args[2..].join(" "); // query might have spaces
                searches.insert(name.clone(), query);

                let new_json = serde_json::to_string(&searches)?;
                db.set_preference("saved_searches", &new_json)?;
                println!("Saved search '{}' added.", name);
            }
            "remove" => {
                if args.len() < 2 {
                    return Err(anyhow!("Usage: saved_searches remove <name>"));
                }
                let name = &args[1];
                if searches.remove(name).is_some() {
                    let new_json = serde_json::to_string(&searches)?;
                    db.set_preference("saved_searches", &new_json)?;
                    println!("Saved search '{}' removed.", name);
                } else {
                    println!("Saved search '{}' not found.", name);
                }
            }
            _ => {
                return Err(anyhow!("Unknown action: {}", action));
            }
        }

        Ok(())
    }
}
