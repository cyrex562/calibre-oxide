use anyhow::Result;

pub struct CmdRemoveFormat;

impl CmdRemoveFormat {
    pub fn new() -> Self {
        CmdRemoveFormat
    }

    pub fn run(&self, library: &mut crate::Library, args: &[String]) -> Result<()> {
        if args.len() != 2 {
            return Err(anyhow::anyhow!("Usage: remove_format <book_id> <fmt>"));
        }
        let book_id: i32 = args[0]
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid book_id"))?;
        let fmt = &args[1];

        println!(
            "CmdRemoveFormat running for book_id: {}, fmt: {}",
            book_id, fmt
        );
        library.remove_format(book_id, fmt)?;
        println!("Format {} removed from book {}", fmt, book_id);
        Ok(())
    }
}
