#!/usr/bin/env bash
set -euo pipefail

# 1. Configuration
MODEL="${1:-llama3.2}" # Default model if none provided
PROMPT="Write a short paragraph explaining the theory of relativity."
URL="http://localhost:11434/api/generate"

echo "============================================="
echo "🚀 Benchmarking Ollama Model: $MODEL"
echo "============================================="

# 2. Execute cURL and capture JSON response
RESPONSE=$(curl -s -X POST "$URL" \
  -H 'Content-Type: application/json' \
  -d "$(jq -n --arg m "$MODEL" --arg p "$PROMPT" '{model: $m, prompt: $p, stream: false, options: {temperature: 0}}')")

# Check if the response contains an error
if echo "$RESPONSE" | grep -q '"error"'; then
  echo "❌ Error from Ollama:"
  echo "$RESPONSE" | jq -r '.error'
  exit 1
fi

# 3. Parse Server-Side Metrics (Values are provided by Ollama in nanoseconds)
LOAD_NS=$(echo "$RESPONSE" | jq '.load_duration')
PP_NS=$(echo "$RESPONSE" | jq '.prompt_eval_duration')
PP_TOKENS=$(echo "$RESPONSE" | jq '.prompt_eval_count')
TG_NS=$(echo "$RESPONSE" | jq '.eval_duration')
TG_TOKENS=$(echo "$RESPONSE" | jq '.eval_count')

# 4. Compute standard metrics
LOAD_MS=$(echo "scale=2; $LOAD_NS / 1000000" | bc)
TTFT_MS=$(echo "scale=2; $PP_NS / 1000000" | bc)

# Guard against zero divisions if response is instant or empty
if [ "$PP_NS" -gt 0 ] && [ "$PP_TOKENS" -gt 0 ]; then
  PP_SPEED=$(echo "scale=2; $PP_TOKENS / ($PP_NS / 1000000000)" | bc)
else
  PP_SPEED="0.00"
fi

if [ "$TG_NS" -gt 0 ] && [ "$TG_TOKENS" -gt 0 ]; then
  TG_SPEED=$(echo "scale=2; $TG_TOKENS / ($TG_NS / 1000000000)" | bc)
else
  TG_SPEED="0.00"
fi

# 5. Output Results
echo "📊 PERFORMANCE REPORT:"
echo "---------------------------------------------"
echo "Model Load Time:       ${LOAD_MS} ms (Cold start overhead)"
echo "Time to First Token:   ${TTFT_MS} ms (TTFT)"
echo "Prompt Processing:     ${PP_SPEED} tokens/sec (${PP_TOKENS} tokens evaluated)"
echo "Token Generation:      ${TG_SPEED} tokens/sec (${TG_TOKENS} tokens generated)"
echo "---------------------------------------------"

