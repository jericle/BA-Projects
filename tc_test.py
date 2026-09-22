#!/usr/bin/env python3
"""Test OpenAI-compatible LLM inference with advanced metrics tracking."""

import argparse
import json
import requests
import os
import time
import shutil
from typing import Optional, List

def parse_args():
    parser = argparse.ArgumentParser(description="Test OpenAI-compatible LLM inference")
    parser.add_argument("--url", "-u", type=str, default="http://10.0.0.2:52415", help="Full LLM server URL")
    parser.add_argument("--model", "-m", type=str, default="mlx-community/Qwen3.6-35B-A3B-4bit", help="LLM model name")
    parser.add_argument("--test-all", action="store_true", help="Run all tests")
    parser.add_argument("--verbose", "-v", action="store_true", help="Display detailed metrics")
    return parser.parse_args()

def make_llm_call(api_url: str, model: str, messages: List[dict], tools: Optional[list] = None) -> tuple:
    endpoint = f"{api_url}/v1/chat/completions"
    # Added temperature=0.7 to ensure random joke generation
    payload = {"model": model, "messages": messages, "tools": tools if tools else [], "stream": False, "temperature": 0.7}
    
    start_time = time.time()
    try:
        response = requests.post(endpoint, json=payload, timeout=300).json()
        latency = time.time() - start_time
        return response, latency
    except Exception as e:
        print(f"\n❌ Error making API call: {e}")
        raise

def execute_tool_response(response: dict) -> tuple:
    choices = response.get("choices", [])
    if not choices: return "", "[No choices returned]"
        
    message = choices[0].get("message", {})
    tool_calls = message.get("tool_calls", [])
    if not tool_calls: return "", "[No tool calls generated]"
        
    output_summary = ""
    details = ""

    for call in tool_calls:
        func_name = call.get("function", {}).get("name")
        args_str = call.get("function", {}).get("arguments", "{}")
        
        try:
            func_args = json.loads(args_str) if isinstance(args_str, str) else args_str
            if func_name == "create_directory":
                path = func_args.get("path")
                os.makedirs(path, exist_ok=True)
                output_summary += f"Directory {path} created. "; details = f"[{path}]"
            elif func_name == "write_file":
                path = func_args.get("path"); content = func_args.get("content")
                dir_name = os.path.dirname(path)
                if dir_name: os.makedirs(dir_name, exist_ok=True)
                with open(path, "w") as f: f.write(content)
                output_summary += f"File {path} written. "; details = f"[{path}]"
            elif func_name == "read_file":
                path = func_args.get("path")
                with open(path, "r") as f: content = f.read()
                output_summary += f"File {path} read. "
                formatted_content = content.replace('\n', '\n    ')
                details = f"[{path}]\n    --- Joke Content ---\n    {formatted_content}\n    --------------------"
            elif func_name == "delete_file":
                path = func_args.get("path")
                if os.path.exists(path): os.remove(path)
                output_summary += f"File {path} deleted. "; details = f"[{path}]"
            elif func_name == "delete_directory":
                path = func_args.get("path")
                if os.path.exists(path): shutil.rmtree(path)
                output_summary += f"Directory {path} deleted. "; details = f"[{path}]"
        except Exception as e:
            print(f"    ❌ Error executing tool {func_name}: {e}")
            
    return output_summary, details

def main():
    args = parse_args()
    api_url = args.url.rstrip('/')

    tools = [
        {"type": "function", "function": {"name": "create_directory", "description": "Create a directory", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}},
        {"type": "function", "function": {"name": "write_file", "description": "Write to a file", "parameters": {"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}}},
        {"type": "function", "function": {"name": "read_file", "description": "Read a file", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}},
        {"type": "function", "function": {"name": "delete_file", "description": "Delete a file", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}},
        {"type": "function", "function": {"name": "delete_directory", "description": "Delete a directory", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}}
    ]

    test_cases = [
        ("Create a directory named 'jokes_folder'", "create_directory"),
        ("Write a 3-line random dirtyjoke to 'jokes_folder/joke.txt'", "write_file"),
        ("Read the contents of 'jokes_folder/joke.txt'", "read_file"),
        ("Delete the file 'jokes_folder/joke.txt'", "delete_file"),
        ("Delete the directory 'jokes_folder'", "delete_directory")
    ]

    messages = []
    total_latency = 0
    max_context_used = 0
    tps_list = []

    for prompt, test_name in test_cases:
        print(f"🚀 Running: {test_name}...", end="", flush=True)
        messages.append({"role": "user", "content": prompt})
        
        try:
            result, latency = make_llm_call(api_url, args.model, messages, tools)
            total_latency += latency
            
            usage = result.get("usage", {})
            completion_tokens = usage.get("completion_tokens", 1)
            total_tokens = usage.get("total_tokens", 0)
            
            max_context_used = max(max_context_used, total_tokens)
            
            tps = completion_tokens / latency if latency > 0 else 0
            tps_list.append(tps)
            
            choices = result.get("choices", [])
            assistant_message = choices[0].get("message", {}) if choices else {}
            messages.append(assistant_message)
            
            tool_output, details = execute_tool_response(result)
            if tool_output: messages.append({"role": "system", "content": tool_output})
            
            if details: print(f" {details}", end="", flush=True)
            print(" ✅")
                
        except Exception as e:
            print(" ❌")
            print(f"   Error: {e}")

    if args.verbose and tps_list:
        print("\n" + "="*40)
        print("📊 Final Aggregated Metrics")
        print("="*40)
        print(f"1. Time to First Token (TTFT) : Not available (Non-streaming)")
        print(f"2. Tokens Per Second (TPS)    : {sum(tps_list)/len(tps_list):.2f} tokens/s")
        print(f"3. Latency (Total)            : {total_latency:.2f}s")
        print(f"4. Cost per 1M Tokens         : $0.00 (Local Model)")
        print(f"5. Context Utilization        : {max_context_used} / 8192 tokens ({(max_context_used/8192)*100:.2f}%)")
        print("="*40)

if __name__ == "__main__":
    main()
