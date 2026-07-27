import socket
import struct
from enum import IntEnum

import xxhash

VAL_SIZE = 36


class Op(IntEnum):
    GET = 1
    UPDATE = 2


class KVClient:
    def __init__(self, host="0.0.0.0", port=8080):
        self.host = host
        self.port = port

        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.settimeout(1)

    def connect(self):
        try:
            self.sock.connect((self.host, self.port))
        except Exception as e:
            print(f"Error connecting to server: {e}")
            self.sock.close()
            raise

    def get(self, bundle_id, idfa) -> str:
        key: bytes = xxhash.xxh3_64_digest(f"{bundle_id}{idfa}".encode())
        packet = struct.pack("B", Op.GET) + key

        self.sock.sendto(packet, (self.host, self.port))
        return self.sock.recv(VAL_SIZE).decode()

    def update(self, bundle_id, idfa, idfv) -> str:
        key: bytes = xxhash.xxh3_64_digest(f"{bundle_id}{idfa}".encode())
        packet = struct.pack("B", Op.UPDATE) + key + idfv.encode()
        self.sock.sendto(packet, (self.host, self.port))
        return self.sock.recv(VAL_SIZE).decode()


client = KVClient()

bundle_id = "com.example.app"
idfa = "AEBE52E7-03EE-455A-B3C4-E57283966239"
idfv = "F325G3GB-12FC-352F-C6C3-DZ52F0F690D8"
