# Owned public TLS fixtures

These certificates and the server key are deliberately public test data, generated for Waifu Studio tests. They are unrelated to user credentials or any service certificate. The CA key is temporary and is not retained. Validity is 2020–2120. The normal peer is valid for `127.0.0.1`; the negative peer is valid for `127.0.0.2`, using the same fixture key and issuer to isolate name verification.

Tests add the fixture CA to individual Rust client configurations only. Production defaults and OS trust stores are unchanged. Tests read DER files directly and need no OpenSSL installation. To regenerate the complete set, run `python3 tests/fixtures/tls/generate.py` with OpenSSL available. Owned fixture material follows the repository MIT license.
