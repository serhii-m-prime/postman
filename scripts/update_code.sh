#!/bin/bash

OUTPUT_FILE="rust_code_base.txt"

echo "=== Збирання кодової бази для NotebookLM... ==="
cd ..;
> "$OUTPUT_FILE"

find . -type f \( -name "*.rs" -o -name "Cargo.toml" -o -name "config.example.yaml" -o -name "Makefile" \) ! -path "*/.*" ! -path "*/target/*" | while read -r file; do
    echo "Adding: $file"
    echo "=== FILE: $file ===" >> "$OUTPUT_FILE"
    cat "$file" >> "$OUTPUT_FILE"
    echo -e "\n\n" >> "$OUTPUT_FILE"
done

echo "=== Done: '$OUTPUT_FILE' . ==="