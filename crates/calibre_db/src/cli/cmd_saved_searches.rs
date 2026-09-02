use crate::Library;
use anyhow::{anyhow, Result};

pub struct CmdSavedSearches;

impl CmdSavedSearches {
    pub fn new() -> Self {
        CmdSavedSearches
    }

    pub fn run(&self, db: &mut Library, args: &[String]) -> Result<()> {
        if args.is_empty() {
            return Err(anyhow!(
                "Usage: saved_searches list|add|remove|rename [name] [query]"
            ));
        }

        let action = &args[0];

        match action.as_str() {
            "list" => {
                let names = db.saved_search_names()?;
                if names.is_empty() {
                    println!("No saved searches found.");
                } else {
                    println!("{:<20} {}", "Name", "Query");
                    println!("{:<20} {}", "----", "-----");
                    for name in names {
                        let query = db.saved_search_lookup(&name)?.unwrap_or_default();
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
                db.saved_search_add(name, &query)?;
                println!("Saved search '{}' added.", name);
            }
            "remove" => {
                if args.len() < 2 {
                    return Err(anyhow!("Usage: saved_searches remove <name>"));
                }
                let name = &args[1];
                if db.saved_search_lookup(name)?.is_some() {
                    db.saved_search_delete(name)?;
                    println!("Saved search '{}' removed.", name);
                } else {
                    println!("Saved search '{}' not found.", name);
                }
            }
            "rename" => {
                if args.len() < 3 {
                    return Err(anyhow!("Usage: saved_searches rename <old-name> <new-name>"));
                }
                let (old_name, new_name) = (&args[1], &args[2]);
                if db.saved_search_lookup(old_name)?.is_some() {
                    db.saved_search_rename(old_name, new_name)?;
                    println!("Saved search '{}' renamed to '{}'.", old_name, new_name);
                } else {
                    println!("Saved search '{}' not found.", old_name);
                }
            }
            _ => {
                return Err(anyhow!("Unknown action: {}", action));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_args_is_a_usage_error() {
        let mut db = Library::open_test().unwrap();
        assert!(CmdSavedSearches::new().run(&mut db, &[]).is_err());
    }

    #[test]
    fn add_then_list_then_remove_round_trips() {
        let mut db = Library::open_test().unwrap();
        let cmd = CmdSavedSearches::new();

        cmd.run(&mut db, &args(&["add", "scifi", "tag:scifi"])).unwrap();
        assert_eq!(db.saved_search_lookup("scifi").unwrap().as_deref(), Some("tag:scifi"));

        cmd.run(&mut db, &args(&["list"])).unwrap();

        cmd.run(&mut db, &args(&["remove", "scifi"])).unwrap();
        assert_eq!(db.saved_search_lookup("scifi").unwrap(), None);
    }

    #[test]
    fn removing_an_unknown_name_does_not_error() {
        let mut db = Library::open_test().unwrap();
        // Matches upstream's own forgiving behavior: reports "not
        // found" rather than failing the command.
        assert!(CmdSavedSearches::new().run(&mut db, &args(&["remove", "nonexistent"])).is_ok());
    }

    #[test]
    fn rename_moves_an_existing_search() {
        let mut db = Library::open_test().unwrap();
        let cmd = CmdSavedSearches::new();
        cmd.run(&mut db, &args(&["add", "old", "tag:x"])).unwrap();
        cmd.run(&mut db, &args(&["rename", "old", "new"])).unwrap();
        assert_eq!(db.saved_search_lookup("old").unwrap(), None);
        assert_eq!(db.saved_search_lookup("new").unwrap().as_deref(), Some("tag:x"));
    }

    #[test]
    fn an_unknown_action_is_an_error() {
        let mut db = Library::open_test().unwrap();
        assert!(CmdSavedSearches::new().run(&mut db, &args(&["bogus"])).is_err());
    }
}
