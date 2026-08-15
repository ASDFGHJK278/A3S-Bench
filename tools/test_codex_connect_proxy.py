#!/usr/bin/env python3
"""Unit tests for the restricted Codex CONNECT proxy runtime asset."""

from __future__ import annotations

import importlib.util
import socket
import ssl
import sys
import threading
import time
import unittest
from pathlib import Path
from unittest import mock


ASSET = Path(__file__).resolve().parents[1] / "runtime_assets" / "codex_connect_proxy.py"
SPEC = importlib.util.spec_from_file_location("codex_connect_proxy", ASSET)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot import {ASSET}")
proxy = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = proxy
SPEC.loader.exec_module(proxy)


def make_client_hello(server_hostname: str | None) -> bytes:
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    incoming = ssl.MemoryBIO()
    outgoing = ssl.MemoryBIO()
    connection = context.wrap_bio(
        incoming, outgoing, server_side=False, server_hostname=server_hostname
    )
    with contextlib_suppress(ssl.SSLWantReadError):
        connection.do_handshake()
    return outgoing.read()


class contextlib_suppress:
    def __init__(self, exception: type[BaseException]) -> None:
        self.exception = exception

    def __enter__(self) -> None:
        return None

    def __exit__(self, kind: object, value: object, traceback: object) -> bool:
        return isinstance(value, self.exception)


class AuthorityTests(unittest.TestCase):
    def test_allows_only_exact_control_plane_hosts_and_content_pattern(self) -> None:
        for authority, expected in (
            ("chatgpt.com:443", "chatgpt.com"),
            ("AB.CHATGPT.COM:443", "ab.chatgpt.com"),
            ("api.openai.com:443", "api.openai.com"),
            ("auth.openai.com:443", "auth.openai.com"),
            ("sdmntprwestus-1.oaiusercontent.com:443", "sdmntprwestus-1.oaiusercontent.com"),
        ):
            with self.subTest(authority=authority):
                self.assertEqual(proxy.validate_authority(authority), (expected, 443))

    def test_rejects_ports_literals_userinfo_urls_and_suffixes(self) -> None:
        rejected = (
            "api.openai.com:80",
            "api.openai.com:0443",
            "127.0.0.1:443",
            "[2606:4700::1111]:443",
            "user@api.openai.com:443",
            "https://api.openai.com:443",
            "api.openai.com.evil.example:443",
            "evil-api.openai.com:443",
            "api.openai.com.:443",
            "sdmntpr.oaiusercontent.com:443",
            "sdmntprfoo.oaiusercontent.com.evil:443",
            "sdmntprfoo-.oaiusercontent.com:443",
            "api.openai.com",
        )
        for authority in rejected:
            with self.subTest(authority=authority):
                with self.assertRaises(proxy.ProxyRequestError):
                    proxy.validate_authority(authority)

    def test_rejects_non_ascii_and_control_characters(self) -> None:
        for authority in ("api.openai.com\x00:443", "api.openai.com\t:443", "é.example:443"):
            with self.subTest(authority=authority):
                with self.assertRaises(proxy.ProxyRequestError):
                    proxy.validate_authority(authority)


class RequestTests(unittest.TestCase):
    def test_parses_connect_without_inspecting_sensitive_header_values(self) -> None:
        request = (
            b"CONNECT api.openai.com:443 HTTP/1.1\r\n"
            b"Host: api.openai.com:443\r\n"
            b"Proxy-Authorization: Bearer secret-not-for-output\r\n\r\n"
        )
        self.assertEqual(proxy.parse_connect_request(request), ("api.openai.com", 443))

    def test_rejects_absolute_http_invalid_ascii_and_early_tunnel_data(self) -> None:
        rejected = (
            b"GET https://api.openai.com/ HTTP/1.1\r\nHost: api.openai.com\r\n\r\n",
            b"CONNECT api.openai.com:443 HTTP/1.1\r\nX-Test: \xff\r\n\r\n",
            b"CONNECT api.openai.com:443 HTTP/1.1\r\nX-Test:\tvalue\r\n\r\n",
            b"CONNECT api.openai.com:443 HTTP/1.1\r\nHost: api.openai.com\r\n\r\nearly",
        )
        for request in rejected:
            with self.subTest(request=request[:20]):
                with self.assertRaises(proxy.ProxyRequestError):
                    proxy.parse_connect_request(request)

    def test_enforces_header_size_limit(self) -> None:
        request = b"CONNECT api.openai.com:443 HTTP/1.1\r\nX: " + (
            b"a" * proxy.MAX_HEADER_BYTES
        ) + b"\r\n\r\n"
        with self.assertRaises(proxy.ProxyRequestError):
            proxy.parse_connect_request(request)


class BindingTests(unittest.TestCase):
    ROUTES = """Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT
eth0 00000000 010011AC 0003 0 0 100 00000000 0 0 0
eth1 00000000 010012AC 0003 0 0 10 00000000 0 0 0
eth0 000011AC 00000000 0001 0 0 0 0000FFFF 0 0 0
"""

    def test_selects_only_non_default_private_interface(self) -> None:
        public = proxy.default_route_interface(self.ROUTES)
        self.assertEqual(public, "eth1")
        self.assertEqual(
            proxy.select_internal_bind_address(
                public,
                [
                    (1, "lo", "127.0.0.1"),
                    (2, "eth0", "172.17.0.2"),
                    (3, "eth1", "172.18.0.2"),
                ],
            ),
            "172.17.0.2",
        )

    def test_fails_closed_for_ambiguous_routes_or_internal_interfaces(self) -> None:
        ambiguous_routes = self.ROUTES.replace(" 100 ", " 10 ")
        with self.assertRaises(OSError):
            proxy.default_route_interface(ambiguous_routes)
        with self.assertRaises(OSError):
            proxy.select_internal_bind_address(
                "eth2",
                [(2, "eth0", "172.17.0.2"), (3, "eth1", "172.18.0.2")],
            )

    def test_explicit_listen_rejects_wildcard_and_non_loopback(self) -> None:
        for address in ("0.0.0.0", "172.17.0.2", "::1"):
            with self.subTest(address=address):
                with mock.patch.object(sys, "argv", ["proxy", "--listen", address]):
                    with self.assertRaises(SystemExit):
                        proxy.main()


class ClientHelloTests(unittest.TestCase):
    HOST = "api.openai.com"

    def read(self, hello: bytes, expected: str = HOST) -> bytes:
        proxy_socket, peer = socket.socketpair()
        try:
            peer.sendall(hello)
            peer.shutdown(socket.SHUT_WR)
            return proxy.read_authorized_client_hello(proxy_socket, expected)
        finally:
            proxy_socket.close()
            peer.close()

    def test_accepts_matching_sni_and_preserves_exact_preface(self) -> None:
        hello = make_client_hello(self.HOST)
        self.assertEqual(self.read(hello), hello)

    def test_accepts_client_hello_split_across_tls_records(self) -> None:
        hello = make_client_hello(self.HOST)
        record_size = int.from_bytes(hello[3:5], "big")
        payload = hello[5 : 5 + record_size]
        split = min(37, len(payload) - 1)

        def record(part: bytes) -> bytes:
            return hello[:3] + len(part).to_bytes(2, "big") + part

        fragmented = record(payload[:split]) + record(payload[split:])
        self.assertEqual(self.read(fragmented), fragmented)

    def test_rejects_missing_mismatched_and_malformed_sni_before_upstream(self) -> None:
        rejected = (
            (make_client_hello(None), self.HOST),
            (make_client_hello("auth.openai.com"), self.HOST),
            (b"\x16\x03\x01\x00\x01\x02", self.HOST),
        )
        for hello, expected in rejected:
            with self.subTest(size=len(hello)):
                with self.assertRaises(proxy.ProxyRequestError):
                    self.read(hello, expected)

    def test_enforces_client_hello_record_size(self) -> None:
        oversized = b"\x16\x03\x01" + (proxy.MAX_TLS_RECORD_BYTES + 1).to_bytes(2, "big")
        with self.assertRaises(proxy.ProxyRequestError):
            self.read(oversized)


class RelayTests(unittest.TestCase):
    def setUp(self) -> None:
        self.client, self.client_peer = socket.socketpair()
        self.upstream, self.upstream_peer = socket.socketpair()
        self.client_peer.settimeout(2.0)
        self.upstream_peer.settimeout(2.0)

    def tearDown(self) -> None:
        for sock in (self.client, self.client_peer, self.upstream, self.upstream_peer):
            sock.close()

    @staticmethod
    def receive_exact(sock: socket.socket, size: int) -> bytes:
        data = bytearray()
        while len(data) < size:
            chunk = sock.recv(size - len(data))
            if not chunk:
                break
            data.extend(chunk)
        return bytes(data)

    def test_relays_preface_and_bidirectional_data_without_blocking_writes(self) -> None:
        preface = b"validated-client-hello"
        thread = threading.Thread(
            target=proxy.relay_tunnel,
            args=(self.client, self.upstream, preface),
            daemon=True,
        )
        thread.start()
        self.assertEqual(self.receive_exact(self.upstream_peer, len(preface)), preface)
        request = b"request" * 16384
        response = b"response" * 16384
        self.client_peer.sendall(request)
        self.assertEqual(self.receive_exact(self.upstream_peer, len(request)), request)
        self.upstream_peer.sendall(response)
        self.assertEqual(self.receive_exact(self.client_peer, len(response)), response)
        self.client_peer.shutdown(socket.SHUT_WR)
        self.upstream_peer.shutdown(socket.SHUT_WR)
        thread.join(timeout=2.0)
        self.assertFalse(thread.is_alive())

    def test_idle_timeout_and_preface_buffer_limit_are_bounded(self) -> None:
        with mock.patch.object(proxy, "IDLE_TIMEOUT_SECONDS", 0.02):
            started = time.monotonic()
            proxy.relay_tunnel(self.client, self.upstream, b"")
            self.assertLess(time.monotonic() - started, 0.5)
        with self.assertRaises(proxy.ProxyRequestError):
            proxy.relay_tunnel(
                self.client,
                self.upstream,
                b"x" * (proxy.MAX_DIRECTION_BUFFER_BYTES + 1),
            )


class ResolutionTests(unittest.TestCase):
    def test_retains_only_global_dns_answers(self) -> None:
        answers = [
            (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("127.0.0.1", 443)),
            (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("10.0.0.2", 443)),
            (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("8.8.8.8", 443)),
            (socket.AF_INET6, socket.SOCK_STREAM, 6, "", ("2606:4700:4700::1111", 443, 0, 0)),
        ]
        with mock.patch.object(proxy, "_getaddrinfo_with_timeout", return_value=answers):
            targets = proxy.resolve_global_targets("api.openai.com", 443)
        self.assertEqual(
            targets,
            [
                (socket.AF_INET, ("8.8.8.8", 443)),
                (socket.AF_INET6, ("2606:4700:4700::1111", 443, 0, 0)),
            ],
        )

    def test_rejects_dns_answers_when_none_are_global(self) -> None:
        answers = [
            (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("169.254.169.254", 443)),
            (socket.AF_INET6, socket.SOCK_STREAM, 6, "", ("::1", 443, 0, 0)),
        ]
        with mock.patch.object(proxy, "_getaddrinfo_with_timeout", return_value=answers):
            with self.assertRaises(OSError):
                proxy.resolve_global_targets("api.openai.com", 443)

    def test_dns_timeout_terminates_and_closes_resolver_process(self) -> None:
        class Endpoint:
            def poll(self, timeout: float) -> bool:
                self.timeout = timeout
                return False

            def close(self) -> None:
                self.closed = True

        class Process:
            alive = True
            terminated = False
            closed = False

            def start(self) -> None:
                self.started = True

            def join(self, timeout: float) -> None:
                self.timeout = timeout

            def is_alive(self) -> bool:
                return self.alive

            def terminate(self) -> None:
                self.terminated = True
                self.alive = False

            def kill(self) -> None:
                self.alive = False

            def close(self) -> None:
                self.closed = True

        receiver = Endpoint()
        sender = Endpoint()
        process = Process()
        context = mock.Mock()
        context.Pipe.return_value = (receiver, sender)
        context.Process.return_value = process
        with (
            mock.patch.object(proxy.multiprocessing, "get_context", return_value=context),
            mock.patch.object(proxy, "CONNECT_TIMEOUT_SECONDS", 0.001),
        ):
            with self.assertRaises(TimeoutError):
                proxy._getaddrinfo_with_timeout("api.openai.com", 443)
        self.assertTrue(process.terminated)
        self.assertTrue(process.closed)


if __name__ == "__main__":
    unittest.main()
