#!/bin/bash

MODEL_ID="mlx-community/Qwen2.5-Coder-7B-Instruct-4bit"

echo "Using model: $MODEL_ID"

curl -s http://10.0.0.2:52415/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d "{
    \"model\": \"$MODEL_ID\",
    \"messages\": [
      {
        \"role\": \"user\",
        \"content\": \"Write the text 'hello world' to a file named test.txt using the write_file tool.\"
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
