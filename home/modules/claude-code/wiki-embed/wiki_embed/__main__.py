import argparse
import logging
import os
from pathlib import Path
import socket
import sys

from .protocol import MAX_LINE, decode_request, encode_message


def main():
    parser = argparse.ArgumentParser(description="Resident semantic retrieval for claude-wiki")
    parser.add_argument("--client", metavar="JSON", help="send one JSON request and print the response")
    parser.add_argument("--socket", type=Path, help="override the runtime socket path")
    parser.add_argument("--wiki-dir", type=Path,
                        default=Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share")) / "claude-wiki")
    args = parser.parse_args()
    # Apply before numpy/torch imports, including to native thread pools. The
    # always-on CPU fallback should leave cores available for the foreground app.
    for key in ("OMP_NUM_THREADS", "MKL_NUM_THREADS", "OPENBLAS_NUM_THREADS"):
        os.environ.setdefault(key, "4")
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")
    from .daemon import default_socket, serve

    try:
        path = args.socket or default_socket()
        if args.client is not None:
            request = decode_request(args.client.encode("utf-8") + b"\n")
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
                connection.settimeout(5)
                connection.connect(str(path))
                connection.sendall(encode_message(request))
                with connection.makefile("rb") as stream:
                    response = stream.readline(MAX_LINE + 1)
            if not response.endswith(b"\n") or len(response) > MAX_LINE:
                raise ValueError("invalid or incomplete response")
            sys.stdout.write(response.decode("utf-8"))
        else:
            logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
            serve(args.wiki_dir, path, os.environ.get("WIKI_EMBED_NO_GPU") == "1")
    except (OSError, ValueError) as error:
        print(f"wiki-embed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
