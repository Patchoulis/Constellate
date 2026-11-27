"""Embed every paper (title + abstract) with a local embedding model; vectors are kept in the library.

Only papers the library has no vector for (same model, same id, same text) are embedded, unless `redo`.

Models are loaded by hand: config -> empty model -> weights from the model's safetensors file.  The usual
`from_pretrained` (and sentence-transformers, which calls it) crashes the interpreter on this machine's
Python 3.15 build; the manual route gives the same weights.  What a hand-loaded model does not know is how its
token vectors become one vector per text, so that is stated here per model (POOLING), as its model card gives it.
"""
import numpy as np

from .library import Embeddings, paper_text

MAX_TOKENS = 512

# How each known model pools: "last" = the final token (decoder-style embedders), "cls" = the first token,
# "mean" = the average over real tokens.  A model not listed is pooled by the mean, the most common choice.
POOLING = {
    "Qwen/Qwen3-Embedding-0.6B": "last",
    "BAAI/bge-base-en-v1.5": "cls",
    "BAAI/bge-large-en-v1.5": "cls",
    "allenai/specter2_base": "cls",
    "sentence-transformers/all-mpnet-base-v2": "mean",
    "sentence-transformers/all-MiniLM-L6-v2": "mean",
}

_loaded = {}


def _weights(name: str) -> dict:
    """The model's tensors: from its safetensors file, or from the older pickle file when it has no other
    (read with weights_only, which refuses anything that is not plain tensors)."""
    import torch
    from huggingface_hub import hf_hub_download
    from huggingface_hub.errors import EntryNotFoundError
    from safetensors.torch import load_file
    try:
        return load_file(hf_hub_download(name, "model.safetensors"))
    except EntryNotFoundError:
        return torch.load(hf_hub_download(name, "pytorch_model.bin"), map_location="cpu", weights_only=True)


class Encoder:
    def __init__(self, name: str):
        import torch
        from transformers import AutoConfig, AutoModel, AutoTokenizer
        self.torch = torch
        self.pooling = POOLING.get(name, "mean")
        self.device = "cuda" if torch.cuda.is_available() else "cpu"
        # the last token is only the text's final token when padding goes on the left
        self.tokenizer = AutoTokenizer.from_pretrained(name, padding_side="left" if self.pooling == "last" else "right")
        model = AutoModel.from_config(AutoConfig.from_pretrained(name))
        weights = _weights(name)
        wanted = set(model.state_dict())
        # some checkpoints store the encoder under a prefix ("bert.embeddings..." for "embeddings...")
        renamed = {next((key[len(prefix):] for prefix in ("", "bert.", "roberta.", "model.", "0.auto_model.")
                         if key.startswith(prefix) and key[len(prefix):] in wanted), key): value.float()
                   for key, value in weights.items()}
        missing = [key for key in model.load_state_dict(renamed, strict=False).missing_keys
                   if "pooler" not in key and "position_ids" not in key]     # unused here / rebuilt by the model
        if missing:
            raise RuntimeError(f"{name}: weights missing for {missing[:5]}")
        self.model = (model.half() if self.device == "cuda" else model).to(self.device).eval()

    def _pool(self, hidden, mask):
        if self.pooling == "last":
            return hidden[:, -1]
        if self.pooling == "cls":
            return hidden[:, 0]
        weights = mask.unsqueeze(-1).to(hidden.dtype)
        return (hidden * weights).sum(dim=1) / weights.sum(dim=1).clamp(min=1)

    def encode(self, texts, batch_size) -> np.ndarray:
        torch = self.torch
        order = np.argsort([-len(t) for t in texts])              # similar lengths together: less padding
        out = np.zeros((len(texts), self.model.config.hidden_size), dtype=np.float32)
        for start in range(0, len(texts), batch_size):
            rows = order[start:start + batch_size]
            batch = self.tokenizer([texts[i] for i in rows], padding=True, truncation=True,
                                   max_length=min(MAX_TOKENS, self.tokenizer.model_max_length),
                                   return_tensors="pt").to(self.device)
            with torch.no_grad():
                pooled = self._pool(self.model(**batch).last_hidden_state.float(), batch["attention_mask"])
            out[rows] = torch.nn.functional.normalize(pooled, dim=-1).cpu().numpy()
        return out


def encoder(name: str) -> Encoder:
    if name not in _loaded:
        _loaded[name] = Encoder(name)
    return _loaded[name]


def embed_papers(papers, model_name: str, redo=False, batch_size=16, log=print) -> np.ndarray:
    """One unit-length float32 row per paper, in the order given."""
    store = Embeddings(model_name)
    missing = papers if redo else [p for p in papers if not store.has(p)]
    log(f"  {len(papers) - len(missing)} found in the library, {len(missing)} to embed with {model_name}")
    if missing:
        model = encoder(model_name)
        chunk = batch_size * 25
        for start in range(0, len(missing), chunk):
            part = missing[start:start + chunk]
            store.add(part, model.encode([paper_text(p) for p in part], batch_size))    # saved chunk by chunk
            log(f"    {min(start + chunk, len(missing))}/{len(missing)}")
    return store.get(papers)
