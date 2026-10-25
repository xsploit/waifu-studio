#!/usr/bin/env python3
"""Regenerate owned public TLS fixtures; OpenSSL is needed only for regeneration."""
from pathlib import Path
import subprocess
import tempfile

output = Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix="waifu-tls-generation-") as temporary:
    root = Path(temporary)
    (root / "index").write_text("")
    (root / "serial").write_text("1000\n")
    (root / "newcerts").mkdir()
    (root / "ca.cnf").write_text("""[ca]
default_ca = CA_default
[CA_default]
database = index
serial = serial
new_certs_dir = newcerts
private_key = ca.key.pem
certificate = ca.cert.pem
default_md = sha256
policy = policy
unique_subject = no
[policy]
commonName = supplied
[ca_extensions]
basicConstraints = critical,CA:TRUE,pathlen:0
keyUsage = critical,keyCertSign,cRLSign
subjectKeyIdentifier = hash
[server_extensions]
basicConstraints = critical,CA:FALSE
keyUsage = critical,digitalSignature
extendedKeyUsage = serverAuth
subjectAltName = IP:127.0.0.1
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always
[wrong_server_extensions]
basicConstraints = critical,CA:FALSE
keyUsage = critical,digitalSignature
extendedKeyUsage = serverAuth
subjectAltName = IP:127.0.0.2
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always
""")
    def run(*args):
        subprocess.run(["openssl", *args], cwd=root, check=True, capture_output=True)
    run("req", "-new", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes",
        "-keyout", "ca.key.pem", "-out", "ca.csr.pem", "-subj", "/CN=Waifu Studio public fixture CA")
    run("ca", "-selfsign", "-batch", "-config", "ca.cnf", "-extensions", "ca_extensions",
        "-startdate", "20200101000000Z", "-enddate", "21200101000000Z", "-in", "ca.csr.pem", "-out", "ca.cert.pem")
    run("req", "-new", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes",
        "-keyout", "server.key.pem", "-out", "server.csr.pem", "-subj", "/CN=Waifu Studio public fixture peer")
    run("ca", "-batch", "-config", "ca.cnf", "-extensions", "server_extensions",
        "-startdate", "20200101000000Z", "-enddate", "21200101000000Z", "-in", "server.csr.pem", "-out", "server.cert.pem")
    run("ca", "-batch", "-config", "ca.cnf", "-extensions", "wrong_server_extensions",
        "-startdate", "20200101000000Z", "-enddate", "21200101000000Z", "-in", "server.csr.pem", "-out", "wrong.cert.pem")
    run("x509", "-in", "wrong.cert.pem", "-outform", "DER", "-out", str(output / "wrong-server.der"))
    run("x509", "-in", "ca.cert.pem", "-outform", "DER", "-out", str(output / "ca.der"))
    run("x509", "-in", "server.cert.pem", "-outform", "DER", "-out", str(output / "server.der"))
    run("pkcs8", "-topk8", "-nocrypt", "-in", "server.key.pem", "-outform", "DER", "-out", str(output / "server-key.der"))
print("Generated public localhost TLS fixtures (2020-2120).")
