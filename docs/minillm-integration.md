# MiniLLM Integration Guide

This guide explains how to use [MiniLLM](../minillm/) -- a GPTQ-quantized local LLM runner -- as a backend provider behind Gaud. Since MiniLLM does not include a built-in API server, an OpenAI-compatible wrapper is required.

## Overview

MiniLLM runs GPTQ 4-bit quantized models on consumer GPUs:

| Model | Base Model | VRAM Required |
|---|---|---|
| `llama-7b-4bit` | LLaMA 7B | ~6 GB |
| `llama-13b-4bit` | LLaMA 13B | ~10 GB |
| `llama-30b-4bit` | LLaMA 30B | ~22 GB |
| `llama-65b-4bit` | LLaMA 65B | ~40 GB |
| `opt-6.7b-4bit` | OPT 6.7B | ~6 GB |

MiniLLM provides a Python API (`minillm.executor`) for loading models and generating text, but no HTTP server. To connect it to Gaud, you need to wrap it in an OpenAI-compatible API server.

## Architecture

```
Client --> Gaud (port 8400) --> MiniLLM API Server (port 8401) --> GPU
              |                       |
              |                       +-- OpenAI-compatible /v1/chat/completions
              |
              +-- Routes "llama-*" and "opt-*" models to minillm provider
```

## Step 1: Install MiniLLM

```bash
cd minillm
pip install -e .
```

Requirements:
- Python 3.10+
- CUDA-capable GPU with sufficient VRAM
- PyTorch with CUDA support
- transformers library

## Step 2: Download Model Weights

```bash
# Download a model (e.g., llama-7b-4bit)
minillm download --model llama-7b-4bit --weights ./weights/llama-7b-4bit.pt
```

Available models for download:
- `llama-7b-4bit` (from HuggingFace: `decapoda-research/llama-7b-hf-int4`)
- `llama-13b-4bit` (from HuggingFace: `decapoda-research/llama-13b-hf-int4`)
- `llama-30b-4bit` (from HuggingFace: `kuleshov/llama-30b-4bit`)
- `llama-65b-4bit` (from HuggingFace: `kuleshov/llama-65b-4bit`)

Note: `opt-6.7b-4bit` does not have a pre-built weights URL. You must quantize it yourself using the GPTQ algorithm.

## Step 3: Create the OpenAI-Compatible API Server

Create `minillm/serve.py` to wrap MiniLLM in a FastAPI server that speaks the OpenAI chat completions protocol:

```python
#!/usr/bin/env python3
"""OpenAI-compatible API server for MiniLLM models."""

import argparse
import time
import uuid
import json
from typing import Optional

import uvicorn
from fastapi import FastAPI, HTTPException
from fastapi.responses import StreamingResponse
from pydantic import BaseModel, Field

from minillm.config import LLM_MODELS, get_llm_config
import minillm.executor as executor

# ---------------------------------------------------------------------------
# Request/Response models (OpenAI-compatible)
# ---------------------------------------------------------------------------

class ChatMessage(BaseModel):
    role: str
    content: Optional[str] = None

class ChatRequest(BaseModel):
    model: str
    messages: list[ChatMessage]
    temperature: float = Field(default=0.7, ge=0.0, le=2.0)
    max_tokens: Optional[int] = Field(default=512, ge=1, le=4096)
    top_p: float = Field(default=0.95, ge=0.0, le=1.0)
    stream: bool = False

class Usage(BaseModel):
    prompt_tokens: int
    completion_tokens: int
    total_tokens: int

class Choice(BaseModel):
    index: int
    message: ChatMessage
    finish_reason: str

class ChatResponse(BaseModel):
    id: str
    object: str = "chat.completion"
    created: int
    model: str
    choices: list[Choice]
    usage: Usage

class ModelInfo(BaseModel):
    id: str
    object: str = "model"
    created: int = 0
    owned_by: str = "minillm"

class ModelsResponse(BaseModel):
    object: str = "list"
    data: list[ModelInfo]

# ---------------------------------------------------------------------------
# Server
# ---------------------------------------------------------------------------

app = FastAPI(title="MiniLLM OpenAI-Compatible Server")

# Global state: loaded model
loaded_model = None
loaded_config = None
loaded_model_name = None

def get_prompt_from_messages(messages: list[ChatMessage]) -> str:
    """Convert chat messages to a single prompt string."""
    parts = []
    for msg in messages:
        if msg.role == "system":
            parts.append(f"System: {msg.content}")
        elif msg.role == "user":
            parts.append(f"User: {msg.content}")
        elif msg.role == "assistant":
            parts.append(f"Assistant: {msg.content}")
    parts.append("Assistant:")
    return "\n".join(parts)

@app.get("/v1/models")
async def list_models():
    """List available models."""
    models = []
    for name in LLM_MODELS:
        models.append(ModelInfo(id=name))
    # Also list loaded model status
    return ModelsResponse(data=models)

@app.get("/health")
async def health():
    return {
        "status": "ok",
        "loaded_model": loaded_model_name,
        "available_models": LLM_MODELS,
    }

@app.post("/v1/chat/completions")
async def chat_completions(request: ChatRequest):
    """OpenAI-compatible chat completions endpoint."""
    global loaded_model, loaded_config, loaded_model_name

    if request.model not in LLM_MODELS:
        raise HTTPException(
            status_code=400,
            detail=f"Unknown model: {request.model}. Available: {LLM_MODELS}"
        )

    # Lazy-load the model (or switch if different model requested)
    if loaded_model_name != request.model:
        config = get_llm_config(request.model)
        weights_path = WEIGHTS_DIR / f"{request.model}.pt"
        if not weights_path.exists():
            raise HTTPException(
                status_code=400,
                detail=f"Weights not found at {weights_path}. Run: minillm download --model {request.model} --weights {weights_path}"
            )
        loaded_model, loaded_config = executor.load_llm(request.model, str(weights_path))
        loaded_model_name = request.model

    # Convert messages to prompt
    prompt = get_prompt_from_messages(request.messages)

    # Generate
    output = executor.generate(
        loaded_model,
        loaded_config,
        prompt,
        min_length=10,
        max_length=request.max_tokens or 512,
        temperature=request.temperature,
        top_k=50,
        top_p=request.top_p,
    )

    # Strip the prompt from the output to get just the completion
    if output.startswith(prompt):
        completion = output[len(prompt):].strip()
    else:
        completion = output.strip()

    # Build response
    prompt_tokens = len(prompt.split())  # Rough estimate
    completion_tokens = len(completion.split())

    return ChatResponse(
        id=f"chatcmpl-{uuid.uuid4().hex[:12]}",
        created=int(time.time()),
        model=request.model,
        choices=[
            Choice(
                index=0,
                message=ChatMessage(role="assistant", content=completion),
                finish_reason="stop",
            )
        ],
        usage=Usage(
            prompt_tokens=prompt_tokens,
            completion_tokens=completion_tokens,
            total_tokens=prompt_tokens + completion_tokens,
        ),
    )

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

from pathlib import Path

WEIGHTS_DIR = Path("./weights")

def main():
    parser = argparse.ArgumentParser(description="MiniLLM OpenAI-compatible server")
    parser.add_argument("--host", default="127.0.0.1", help="Bind address")
    parser.add_argument("--port", type=int, default=8401, help="Port")
    parser.add_argument("--weights-dir", type=str, default="./weights",
                        help="Directory containing model weight files")
    parser.add_argument("--preload", choices=LLM_MODELS, default=None,
                        help="Pre-load a model at startup")
    args = parser.parse_args()

    global WEIGHTS_DIR
    WEIGHTS_DIR = Path(args.weights_dir)

    if args.preload:
        global loaded_model, loaded_config, loaded_model_name
        weights_path = WEIGHTS_DIR / f"{args.preload}.pt"
        print(f"Pre-loading model {args.preload} from {weights_path}...")
        loaded_model, loaded_config = executor.load_llm(args.preload, str(weights_path))
        loaded_model_name = args.preload
        print(f"Model {args.preload} loaded successfully")

    uvicorn.run(app, host=args.host, port=args.port)

if __name__ == "__main__":
    main()
```

Install the additional dependency:

```bash
pip install fastapi uvicorn
```

## Step 4: Run the MiniLLM Server

```bash
# Start with a pre-loaded model
python minillm/serve.py --preload llama-7b-4bit --weights-dir ./weights

# Or start empty and load on first request
python minillm/serve.py --port 8401
```

Verify it's running:

```bash
curl http://localhost:8401/health
# {"status":"ok","loaded_model":"llama-7b-4bit","available_models":["llama-7b-4bit",...]}

curl http://localhost:8401/v1/models
# {"object":"list","data":[{"id":"llama-7b-4bit","object":"model",...}]}
```

## Step 5: Configure Gaud as a Proxy

Gaud doesn't yet have a built-in "generic OpenAI-compatible" provider type. To route requests to the MiniLLM server, you have two options:

### Option A: Add a Custom Provider (Recommended)

Add a new provider implementation to Gaud that forwards requests to any OpenAI-compatible endpoint. This is the cleanest approach and enables full integration with the routing, circuit breaker, and budgeting systems.

The provider would:
1. Accept a `base_url` configuration (e.g., `http://localhost:8401`)
2. Forward `POST /v1/chat/completions` requests as-is (the format is already OpenAI-compatible)
3. Register models reported by `GET /v1/models`

Example config (future):

```toml
[providers.minillm]
type = "openai_compatible"
base_url = "http://localhost:8401"
# No auth needed for local server
```

### Option B: Use a Reverse Proxy

Point clients directly at the MiniLLM server for local models, and at Gaud for cloud models. Less elegant but works immediately.

## Step 6: Web UI Auto-Configuration

The Gaud web UI settings page (`/ui/settings`) can be extended to support local model discovery:

1. **Model Discovery**: The MiniLLM server's `GET /v1/models` endpoint lists all available models. The web UI could poll this endpoint and display available local models alongside cloud providers.

2. **Auto-Detection Flow** (future enhancement):
   - User enters the MiniLLM server URL in the settings page
   - Gaud calls `GET {url}/v1/models` to discover available models
   - Discovered models appear in the dashboard's provider status table
   - User can select which models to enable

3. **Weight Management**: The MiniLLM server's `/health` endpoint reports loaded models and available weights. The web UI could show download progress and VRAM usage.

### Configuration via Settings API

```bash
# Future: register a local provider via the admin API
curl -X PUT http://localhost:8400/admin/settings \
  -H "Authorization: Bearer sk-prx-..." \
  -H "Content-Type: application/json" \
  -d '{"key": "providers.minillm.base_url", "value": "http://localhost:8401"}'
```

## MiniLLM Model Configuration Reference

Each model in MiniLLM is defined by a config class in `minillm/minillm/llms/`:

| Model | HuggingFace Config | Bits | Weights URL |
|---|---|---|---|
| `llama-7b-4bit` | `decapoda-research/llama-7b-hf` | 4 | [HuggingFace](https://huggingface.co/decapoda-research/llama-7b-hf-int4) |
| `llama-13b-4bit` | `decapoda-research/llama-13b-hf` | 4 | [HuggingFace](https://huggingface.co/decapoda-research/llama-13b-hf-int4) |
| `llama-30b-4bit` | `decapoda-research/llama-30b-hf` | 4 | [HuggingFace](https://huggingface.co/kuleshov/llama-30b-4bit) |
| `llama-65b-4bit` | `decapoda-research/llama-65b-hf` | 4 | [HuggingFace](https://huggingface.co/kuleshov/llama-65b-4bit) |
| `opt-6.7b-4bit` | `facebook/opt-6.7b` | 4 | N/A (manual quantization) |

The tokenizer is loaded from the `hf_config_name` HuggingFace model ID via `AutoTokenizer.from_pretrained()`.

## MiniLLM Python API Reference

```python
import minillm.executor as minillm

# Load a model
llm, llm_config = minillm.load_llm("llama-7b-4bit", "./weights/llama-7b-4bit.pt")

# Generate text
output = minillm.generate(
    llm,
    llm_config,
    prompt="Once upon a time",
    min_length=10,
    max_length=200,
    temperature=0.7,
    top_k=50,
    top_p=0.95,
)
print(output)
```

### Key Functions

| Function | Description |
|---|---|
| `load_llm(model, weights)` | Load a quantized model. Returns `(model, config)` tuple. |
| `generate(llm, config, prompt, ...)` | Generate text from a prompt. Returns the full output string including the prompt. |

### CLI Commands

```bash
# Generate text
minillm generate --model llama-7b-4bit --weights ./weights/llama-7b-4bit.pt \
    --prompt "Hello, world" --max-length 100 --temperature 0.7

# Download weights
minillm download --model llama-7b-4bit --weights ./weights/llama-7b-4bit.pt
```

## Hardware Requirements

Based on the system hardware (NVIDIA GTX TITAN X 12GB):

| Model | Fits in VRAM? | Notes |
|---|---|---|
| `llama-7b-4bit` | Yes | ~6 GB, good performance |
| `llama-13b-4bit` | Yes | ~10 GB, tight fit |
| `llama-30b-4bit` | No | ~22 GB, requires multi-GPU or CPU offload |
| `llama-65b-4bit` | No | ~40 GB, requires multi-GPU |
| `opt-6.7b-4bit` | Yes | ~6 GB, good performance |

## Troubleshooting

### CUDA Out of Memory

If you get OOM errors, try a smaller model or reduce `max_tokens`:

```bash
# Use the 7B model instead of 13B
python minillm/serve.py --preload llama-7b-4bit
```

### Slow First Request

The first request triggers model loading, which takes 30-60 seconds. Use `--preload` to load the model at server startup instead.

### Model Weights Not Found

Download weights before starting the server:

```bash
minillm download --model llama-7b-4bit --weights ./weights/llama-7b-4bit.pt
```
