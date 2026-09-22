#!/usr/bin/env python3
"""Test Ollama LLM inference with advanced metrics tracking."""

import argparse
import json
import requests
import os
import time
from typing import Optional, List

def parse_args():
    parser = argparse.ArgumentParser(description="Test Ollama LLM inference")
    parser.add_argument("--local", "-l", type=str, default="11434", help="Local Ollama port")
    parser.add_argument("--model", "-m", type=str, default="qwen2.5:7b", help="Ollama model name")
    parser.add_argument("--test-all", action="store_true", help="Run all tests")
    parser.add_argument("--verbose", "-v", action="store_true", help="Display detailed metrics")
    return parser.parse_args()

def make_ollama_call(api_url: str, model: str, messages: List[dict], tools: Optional[list] = None) -> tuple:
    endpoint = f"{api_url}/api/chat"
    payload = {"model": model, "messages": messages, "tools": tools if tools else [], "stream": False}
    
    start_time = time.time()
    try:
        response = requests.post(endpoint, json=payload, timeout=300).json()
        latency = time.time() - start_time
        return response, latency
    except Exception as e:
        print(f"\n❌ Error making API call: {e}")
        raise

def execute_tool_response(response: dict) -> str:
    message = response.get("message", {})
    tool_calls = message.get("tool_calls", [])
    output_summary = ""

    for call in tool_calls:
        func_name = call.get("function", {}).get("name")
        args_str = call.get("function", {}).get("arguments", "{}")
        
        try:
            func_args = json.loads(args_str) if isinstance(args_str, str) else args_str
            if func_name == "create_directory":
                path = func_args.get("path")
                os.makedirs(path, exist_ok=True)
                output_summary += f"Directory {path} created. "
            elif func_name == "write_file":
                path = func_args.get("path"); content = func_args.get("content")
                dir_name = os.path.dirname(path)
                if dir_name: os.makedirs(dir_name, exist_ok=True)
                with open(path, "w") as f: f.write(content)
                output_summary += f"File {path} written. "
            elif func_name == "read_file":
                path = func_args.get("path")
                with open(path, "r") as f: content = f.read()
                output_summary += f"File {path} read. "
        except Exception as e:
            print(f"    ❌ Error executing tool {func_name}: {e}")
    return output_summary

def main():
    args = parse_args()
    api_url = f"http://localhost:{args.local}"

    tools = [
        {"type": "function", "function": {"name": "create_directory", "description": "Create a directory", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}},
        {"type": "function", "function": {"name": "write_file", "description": "Write to a file", "parameters": {"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}}},
        {"type": "function", "function": {"name": "read_file", "description": "Read a file", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}}
    ]

    test_cases = [
        ("Create a directory named 'jokes_folder'", "create_directory"),
        ("Write a random joke to 'jokes_folder/joke.txt'", "write_file"),
        ("Read the contents of 'jokes_folder/joke.txt'", "read_file")
    ]

    messages = []
    total_latency = 0
    total_tokens = 0
    max_context_used = 0
    ttft_list = []
    tps_list = []

    for prompt, test_name in test_cases:
        print(f"🚀 Running: {test_name}...", end="", flush=True)
        messages.append({"role": "user", "content": prompt})
        
        try:
            result, latency = make_ollama_call(api_url, args.model, messages, tools)
            total_latency += latency
            
            # Calculate Metrics
            eval_count = result.get("eval_count", 1)
            total_tokens += eval_count
            prompt_tokens = result.get("prompt_eval_count", 0)
            max_context_used = max(max_context_used, prompt_tokens + eval_count)
            
            ttft = (result.get('total_duration', 0) - result.get('eval_duration', 0)) / 1e9
            tps = eval_count / (result.get('eval_duration', 1) / 1e9)
            
            ttft_list.append(ttft)
            tps_list.append(tps)
            
            assistant_message = result.get("message", {})
            messages.append(assistant_message)
            
            tool_output = execute_tool_response(result)
            if tool_output: messages.append({"role": "system", "content": tool_output})
            
            print(" ✅")
                
        except Exception as e:
            print(" ❌")
            print(f"   Error: {e}")

    if args.verbose and ttft_list:
        print("\n" + "="*40)
        print("📊 Final Aggregated Metrics")
        print("="*40)
        print(f"1. Time to First Token (TTFT) : {sum(ttft_list)/len(ttft_list):.2f}s (avg)")
        print(f"2. Tokens Per Second (TPS)    : {sum(tps_list)/len(tps_list):.2f} tokens/s")
        print(f"3. Latency (Total)            : {total_latency:.2f}s")
        print(f"4. Cost per 1M Tokens         : $0.00 (Local Model)")
        print(f"5. Context Utilization        : {max_context_used} / 8192 tokens ({(max_context_used/8192)*100:.2f}%)")
        print("="*40)

if __name__ == "__main__":
    main()
