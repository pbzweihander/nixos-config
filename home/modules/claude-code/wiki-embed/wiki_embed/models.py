import gc

import numpy as np

from .core import CPU_MODEL, GPU_MODEL, RERANKER


class Models:
    """Only this adapter imports inference libraries; tests supply a small fake."""

    def __init__(self, tier):
        import torch
        from sentence_transformers import SentenceTransformer
        from transformers import AutoModelForSequenceClassification, AutoTokenizer

        self.tier = tier
        self.encoder = self.reranker = self.tokenizer = None
        self.name = GPU_MODEL if tier == "gpu" else CPU_MODEL
        if tier == "gpu" and not torch.cuda.is_available():
            raise RuntimeError("ROCm is unavailable")
        try:
            self.encoder = SentenceTransformer(
                self.name, device="cuda" if tier == "gpu" else "cpu", local_files_only=True,
                model_kwargs={"dtype": torch.float16 if tier == "gpu" else torch.float32})
            # The same on both tiers: vectors cached by one tier are read by the
            # other, so the passage encoding must not depend on which one built them.
            self.encoder.max_seq_length = 1024
            self.encoder.eval()
            if tier == "gpu":
                self.tokenizer = AutoTokenizer.from_pretrained(RERANKER, local_files_only=True)
                self.reranker = AutoModelForSequenceClassification.from_pretrained(
                    RERANKER, dtype=torch.float16, local_files_only=True).to("cuda").eval()
                # Pay first-use ROCm initialization on the loader thread, before
                # exposing the tier to a latency-sensitive prompt hook.
                self.encode_query("warmup")
                self.rerank("warmup", ["warmup " * 180] * 10)
        except Exception:
            self.close()
            raise

    def _encode(self, texts):
        import torch

        with torch.inference_mode():
            return self.encoder.encode(
                texts, batch_size=8 if self.tier == "gpu" else 2,
                normalize_embeddings=True, show_progress_bar=False,
                convert_to_numpy=True).astype(np.float32)

    # bge-m3 takes no query or passage prefix, on either tier.
    def encode_passages(self, texts):
        return self._encode(texts)

    def encode_query(self, query):
        return self._encode([query])[0]

    def rerank(self, query, texts):
        import torch

        with torch.inference_mode():
            inputs = self.tokenizer([query] * len(texts), texts, padding=True,
                                    truncation=True, max_length=512, return_tensors="pt").to("cuda")
            # The measured cutoff is in logit space, before any sigmoid.
            return self.reranker(**inputs).logits[:, 0].float().cpu().numpy()

    def close(self):
        import torch

        self.encoder = self.reranker = self.tokenizer = None
        gc.collect()
        if self.tier == "gpu":
            torch.cuda.empty_cache()
