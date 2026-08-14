from http.server import BaseHTTPRequestHandler, HTTPServer
import json
import sys
import time
from socketserver import ThreadingMixIn

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18080

class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        size = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(size)
        request = json.loads(body or b"{}")
        if request.get("model") == "fail" and PORT == 18080:
            self.send_response(500)
            self.send_header("content-type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"error":{"message":"transient"}}')
            return
        response = {"ok": True, "provider_port": PORT, "received": request, "usage": {
            "input_tokens": 12, "output_tokens": 4, "total_tokens": 16
        }}
        if request.get("stream"):
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.end_headers()
            if request.get("model") == "long-stream":
                # Exercise the gateway's bounded stream scanner: the terminal
                # event intentionally arrives after the old 2 MiB capture cap.
                self.wfile.write(b"data: " + (b"x" * (3 * 1024 * 1024)) + b"\n\n")
                self.wfile.flush()
            if request.get("model") == "cancel-stream":
                self.wfile.write(b'data: {"type":"response.output_text.delta","delta":"partial"}\n\n')
                self.wfile.flush()
                time.sleep(5)
                return
            if request.get("model") == "anthropic-stream":
                for event in [
                    {"type": "message_start", "message": {"usage": {"input_tokens": 12}}},
                    {"type": "message_delta", "usage": {"output_tokens": 4}},
                    {"type": "message_stop"},
                ]:
                    self.wfile.write(("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n").encode())
                    self.wfile.flush()
                return
            for event in [
                {"type": "response.output_text.delta", "delta": "streamed"},
                {"type": "response.completed", "usage": response["usage"]},
            ]:
                self.wfile.write(("data: " + json.dumps(event) + "\n\n").encode())
                self.wfile.flush()
            return
        encoded = json.dumps(response).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def log_message(self, *_):
        pass

class ThreadingHTTPServer(ThreadingMixIn, HTTPServer):
    daemon_threads = True

ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
