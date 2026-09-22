#!/bin/bash

MODEL_ID="qwen3.5:9b-mlx"

echo "Using model: $MODEL_ID"

curl -s http://localhost:11434/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d "{
    \"model\": \"$MODEL_ID\",
    \"messages\": [
      {
        \"role\": \"user\",
        \"content\": \"Write the text 'hello world' to a file named test.txt in the current directoryusing the write_file tool.\"
      }
    ],
    \"tools\": [
      {
        \"type\": \"function\",
        \"function\": {
          \"name\": \"write_file\",
          \"description\": \"Write content to a file\",
          \"parameters\": {
            \"type\": \"object\",
            \"properties\": {
              \"path\": {
                \"type\": \"string\",
                \"description\": \"The file path to write to\"
              },
              \"content\": {
                \"type\": \"string\",
                \"description\": \"The content to write\"
              }
            },
            \"required\": [\"path\", \"content\"]
          }
        }
      }
    ],
    \"tool_choice\": \"auto\",
    \"stream\": false
  }" | python3 -m json.tool
