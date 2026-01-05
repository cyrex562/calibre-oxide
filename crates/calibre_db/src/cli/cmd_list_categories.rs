use anyhow::Result;

pub struct CmdListCategories;

impl CmdListCategories {
    pub fn new() -> Self {
        CmdListCategories
    }

    pub fn run(&self, library: &crate::Library, args: &[String]) -> Result<()> {
        let csv_output = args.contains(&"--csv".to_string()) || args.contains(&"-c".to_string());
        let categories = library.get_categories()?;

        if csv_output {
            println!("category,name,count");
            for (category_name, items) in &categories {
                for item in items {
                    println!("{},{},{}", category_name, item.name, item.count);
                }
            }
        } else {
            for (category_name, items) in &categories {
                println!("{}:", category_name.to_uppercase());
                for item in items {
                    println!("  {} ({})", item.name, item.count);
                }
            }
        }

        Ok(())
    }
}
