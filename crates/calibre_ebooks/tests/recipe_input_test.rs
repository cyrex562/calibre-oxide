use calibre_ebooks::input::recipe_input::RecipeInput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_recipe_input_placeholder() {
    let temp_dir = tempdir().unwrap();
    let output_dir = temp_dir.path().join("out");
    let input_path = temp_dir.path().join("test.recipe");
    fs::write(&input_path, b"print('Hello')").unwrap();

    let input = RecipeInput::new();
    let book = input.convert(&input_path, &output_dir).unwrap();

    assert!(output_dir.join("index.html").exists());
    let content = fs::read_to_string(output_dir.join("index.html")).unwrap();
    assert!(content.contains("Recipe Execution Not Supported"));

    let titles = book.metadata.get("title");
    assert!(!titles.is_empty());
    assert_eq!(titles[0].value, "Recipe Result");
}
