"""Independent modern pairing verification for the loopback benchmark receiver."""
import binascii
from pyatv.protocols.airplay.server_auth import BaseAirPlayServerAuth
from pyatv.auth.hap_session import HAPSession
from pyatv.auth.hap_srp import hkdf_expand
from pyatv.auth.hap_tlv8 import read_tlv, TlvValue
from pyatv.support.chacha20 import Chacha20Cipher8byteNonce
from pyatv.support.opack import unpack
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

class Reference(BaseAirPlayServerAuth):

    def enable_encryption(self, output_key, input_key):
        self.ready = (output_key, input_key)

    def _m5_setup(self, message, transient):
        secret = binascii.unhexlify(self.session.key)
        key = hkdf_expand('Pair-Setup-Encrypt-Salt', 'Pair-Setup-Encrypt-Info', secret)
        cipher = Chacha20Cipher8byteNonce(key, key)
        inner = read_tlv(cipher.decrypt(message[TlvValue.EncryptedData], nonce=b'PS-Msg05'))
        assert unpack(inner[18])[0] == {'com.apple.ScreenCapture': True}
        signed = hkdf_expand('Pair-Setup-Controller-Sign-Salt', 'Pair-Setup-Controller-Sign-Info', secret) + inner[TlvValue.Identifier] + inner[TlvValue.PublicKey]
        Ed25519PublicKey.from_public_bytes(inner[TlvValue.PublicKey]).verify(inner[TlvValue.Signature], signed)
        self.client_key = inner[TlvValue.PublicKey]
        self.client_id = inner[TlvValue.Identifier]
        response = super()._m5_setup(message, transient)
        return response

    def _m1_verify(self, message):
        self.client_ephemeral = message[TlvValue.PublicKey]
        response = super()._m1_verify(message)
        return response

    def _m3_verify(self, message):
        from cryptography.hazmat.primitives import serialization
        key = hkdf_expand('Pair-Verify-Encrypt-Salt', 'Pair-Verify-Encrypt-Info', self.shared_key)
        cipher = Chacha20Cipher8byteNonce(key, key)
        inner = read_tlv(cipher.decrypt(message[TlvValue.EncryptedData], nonce=b'PV-Msg03'))
        assert inner[TlvValue.Identifier] == self.client_id
        server = self.keys.verify_pub.public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
        Ed25519PublicKey.from_public_bytes(self.client_key).verify(inner[TlvValue.Signature], self.client_ephemeral + self.client_id + server)
        return super()._m3_verify(message)
