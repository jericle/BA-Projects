import json, subprocess, requests

resp = requests.post("http://10.0.0.2:52415/v1/chat/completions", json={
    "model": "mlx-community/Qwen3.6-35B-A3B-4bit",
    "messages": [{"role": "user", "content": "Write the text 'hello world' to a file named test.txt in the current directory using the write_file tool."}],
    "tools": [{
        "type": "function",
        "function": {
            "name": "write_file",
            "description": "Write content to a file",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "The file path to write to"},
                    "content": {"type": "string", "description": "The content to write"}
                },
                "required": ["path", "content"]
            }
        }
    }],
    "tool_choice": "auto"
}).json()

tool_calls = resp["choices"][0]["message"].get("tool_calls", [])
for call in tool_calls:
    if call["function"]["name"] == "write_file":
        args = json.loads(call["function"]["arguments"])
        with open(args["path"], "w") as f:
            f.write(args["content"])
        print(f"Wrote {args['path']}")
