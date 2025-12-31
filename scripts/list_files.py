import argparse
import pathlib


def main():
    parser = argparse.ArgumentParser(description="List files in a directory")
    parser.add_argument("path", type=pathlib.Path, help="Path to directory")
    parser.add_argument(
        "--recursive", action="store_true", help="List files recursively"
    )
    parser.add_argument(
        "--markdown-checklist", action="store_true", help="Output as markdown checklist"
    )
    args = parser.parse_args()

    if args.recursive:
        for file in args.path.rglob("*"):
            if file.is_file():
                if args.markdown_checklist:
                    print(f"- [ ] {file.name}")
                else:
                    print(file)
    else:
        for file in args.path.iterdir():
            if file.is_file():
                if args.markdown_checklist:
                    print(f"- [ ] {file.name}")
                else:
                    print(file)


if __name__ == "__main__":
    main()
