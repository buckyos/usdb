"""Valid public enode fixtures; these reserved endpoints never require network IO."""
PUBLIC_KEY = ("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
              "483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8")
V4 = f"enode://{PUBLIC_KEY}@192.0.2.1:31303"
V6 = f"enode://{PUBLIC_KEY}@[2001:db8::1]:31303"
DNS = f"enode://{PUBLIC_KEY}@seed.example.org:31303"
