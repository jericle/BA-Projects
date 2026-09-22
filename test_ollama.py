#!/usr/bin/env python3
"""Test Ollama LLM inference with tool calling support.

Usage examples:
    python test_ollama.py -p "Hello world"
    python test_ollama.py --local 11434 --model qwen3.5:9b-mlx -p "Say hello"
    python test_ollama.py -l 11434 -m llama3.2 --prompt "Write a poem about Python"
"""

import argparse
import json
import requests
import time
from typing import Optional


def parse_args():
    parser = argparse.ArgumentParser(
        description="Test Ollama LLM inference with tool calling"
    )

    parser.add_argument(
        "--local",
        "-l",
        type=str,
        help="Local Ollama port (default: 11434)",
        default="11434"
    )

    parser.add_argument(
        "--model",
        "-m",
        type=str,
        help="Ollama model name to use",
        default="qwen3.5:9b-mlx"
    )

    parser.add_argument(
        "--prompt",
        "-p",
        type=str,
        help="The prompt to send to the LLM"
    )

    parser.add_argument(
        "--test-all",
        action="store_true",
        help="Run a battery of tests for all defined tools"
    )

    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        help="Enable verbose output with timing details"
    )

    args = parser.parse_args()
    if not args.prompt and not args.test_all:
        parser.error("The following arguments are required: --prompt/-p or --test-all")
    
    return args


def make_ollama_call(
    api_url: str,
    model: str,
    content: str,
    tools: Optional[list] = None,
    tool_choice: str = "auto"
) -> dict:
    """Make a chat completion call to Ollama API with optional tool definitions."""

    payload = {
        "model": model,
        "messages": [{"role": "user", "content": content}],
        "tools": tools if tools else [],
        "tool_choice": tool_choice
    }

    start_time = time.time()
    print(f"📡 Making request to {api_url}...")
    print(f"   Model: {model}")
    print(f"   Prompt: {content[:100]}{'...' if len(content) > 100 else ''}")

    try:
        response = requests.post(api_url, json=payload, timeout=300).json()
        elapsed_total = time.time() - start_time

        # Print timing stats
        print("\n⏱️  Timing Results:")
        print(f"    └─ Total Time: {elapsed_total:.2f}s")

        return response

    except requests.exceptions.Timeout:
        elapsed_total = time.time() - start_time
        print("\n⏱️  Request TIMEOUT after {:.2f}s".format(elapsed_total))
        raise
    except Exception as e:
        elapsed_total = time.time() - start_time
        print(f"\n❌ Error: {e}")
        print(f"   Total Time: {elapsed_total:.2f}s")
        raise


def execute_tool_response(response: dict) -> None:
    """Execute any tool calls returned by Ollama."""
    choices = response.get("choices", [])
    if not choices:
        print("\n✅ No tool calls to execute")
        return

    message = choices[0].get("message", {})
    tool_calls = message.get("tool_calls", [])

    if not tool_calls:
        print("\n✅ No tool calls to execute")
        return

    for idx, call in enumerate(tool_calls):
        func_name = call.get("function", {}).get("name")
        args_str = call.get("function", {}).get("arguments", "{}")
        
        try:
            func_args = json.loads(args_str) if isinstance(args_str, str) else args_str
            
            if func_name == "write_file":
                path = func_args.get("path", f"test_{idx}.txt")
                content = func_args.get("content", "")
                print(f"\n📝 Executing write_file: {path}")
                with open(path, "w") as f:
                    f.write(content)
                print(f"    ✅ File written: {path}")
            
            elif func_name == "get_weather":
                location = func_args.get("location", "Unknown")
                print(f"\n🌤️  Executing get_weather for: {location}")
                print(f"    ✅ Mock Result: 22°C and Sunny in {location}")
                
            elif func_name == "calculate":
                expression = func_args.get("expression", "0")
                print(f"\n🔢 Executing calculate: {expression}")
                try:
                    result = eval(expression, {"__builtins__": None}, {})
                    print(f"    ✅ Result: {result}")
                except Exception as e:
                    print(f"    ❌ Eval Error: {e}")
            else:
                print(f"\n❓ Unknown tool: {func_name} with args {func_args}")

        except Exception as e:
            print(f"    ❌ Error executing tool {func_name}: {e}")


def main():
    args = parse_args()

    # Build API URL based on local/remote configuration
    if args.local == "11434":
        api_url = "http://localhost:11434/v1/chat/completions"
    else:
        api_url = f"http://localhost:{args.local}/v1/chat/completions"
    is_local = True

    print("=" * 60)
    print("🚀 Ollama Test Runner")
    print("=" * 60)
    print(f"\n💻 Target: {api_url}")
    print(f"   ├─ Model: {args.model}")
    print(f"   ├─ Port: {args.local}")
    print(f"   └─ Local: {'Yes ✅' if is_local else 'No (remote)'}")

    tools = [
        {
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
        }
    ]

    try:
        result = make_ollama_call(
            api_url=api_url,
            model=args.model,
            content=args.prompt,
            tools=tools,
            tool_choice="auto"
        )

        print("\n✅ Response received successfully!")
        print("-" * 40)
        print("\n📝 Assistant Response:")
        response_content = result.get("choices", [{}])[0].get("message", {}).get("content", "No content")
        print(response_content)

        # Execute any tool calls if present
        execute_tool_response(result)

    except Exception as e:
        print(f"\n❌ Connection failed: {e}")
        print("\n💡 Hint:")
        if args.local == "11434":
            print("   Are you sure Ollama is running on port 11434?")
            print("   Run: ollama serve &")
        else:
            print(f"   Port {args.local} may not be available.")
            print("   Try: ollama serve")


if __name__ == "__main__":
    main()