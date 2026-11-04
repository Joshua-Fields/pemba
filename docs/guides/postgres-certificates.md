# Pin a managed Postgres server's certificate

pemba verifies a Postgres server's certificate against the platform trust store. Some managed
servers use a certificate that store can never accept, so the connection fails with an error
such as `UnknownIssuer`, `CaUsedAsEndEntity` or "certificate not valid for name". Do not turn TLS
off. Pin the server's exact certificate instead.

**A pinned certificate stops connections when the provider replaces it.** New connections fail
until you update the pin, so only pin if you can respond to that. Read
[when the certificate changes](#when-the-certificate-changes) first.

## Get the certificate

From a machine that can reach the database (OpenSSL 1.1.1 or newer):

```sh
openssl s_client -starttls postgres -connect HOST:PORT </dev/null 2>/dev/null \
  | openssl x509 > server.pem
openssl x509 -in server.pem -noout -subject -issuer -enddate -fingerprint -sha256
```

Do not add `-showcerts`. It can also print the issuer, and pemba refuses more than one
certificate.

This command believes whatever answered it, so it can capture an attacker's certificate. If
you can get a shell on the database itself, compare the fingerprint there:

```sh
psql -tAc 'SHOW ssl_cert_file'            # on the database, as a role allowed to read it
openssl x509 -in THAT_FILE -noout -fingerprint -sha256
```

If you cannot, fetch the certificate from a network you trust, from more than one place, and
compare the results.

## Use it

Pass the PEM text, not a file path. In Python, from an environment variable:

```python
import os
import pemba

engine = pemba.Engine.postgres(
    os.environ["DATABASE_URL"],
    pinned_certificate_pem=os.environ["PG_SERVER_CERT"],
)
```

Use `os.environ[...]`, not `.get`, so a missing variable fails at start-up. In Rust:

```rust
let config = PostgresConfig::default()
    .with_schema("authz")
    .with_pinned_certificate_pem(std::env::var("PG_SERVER_CERT")?);
let store = PostgresStore::open(&dsn, config)?;
```

The variable must keep real line breaks. If a platform stores a literal `\n`, pemba reports
that the value is not valid PEM. A certificate is public, but treat changes to this variable
like changes to a credential: whoever sets it decides which server you trust.

## What it checks

The server must present exactly the bytes you pinned and prove, with a handshake signature,
that it holds the matching private key. The check runs on every connection. The platform trust
store is not used.

Because those bytes are the whole trust decision, pemba does not check the certificate's
issuer, name, CA flag or expiry. A name that does not match the host you connect to is fine.

| `sslmode` in the DSN | Without a pin | With a pin |
|---|---|---|
| `disable` | No TLS | Error when the engine opens |
| `prefer`, or none | TLS if offered, plaintext if not | Acts as `require` |
| `require` | TLS, verified against the trust store | TLS, verified against the pin |

Pin the server's own certificate. Pinning a CA does not trust what it issues: pemba refuses a
server presenting a certificate issued by that CA. For a private CA, install it in the platform
trust store instead.

A pin covers only the engine's own connections. In a foreign write, relationships travel on your
application's connection, with your driver's TLS settings. psycopg's default `sslmode=prefer`,
for example, does not verify the server, and pemba's pin does not change that.

## When the certificate changes

When the provider replaces the certificate, new connections fail with `not the pinned
certificate`. Connections already open keep working until they close, so the failure can
appear gradually, or only after a restart. There is no fallback, and that is deliberate: pemba
cannot tell a replacement from an attacker.

To recover:

1. [Get the new certificate](#get-the-certificate) and check its fingerprint, as above. An
   unexplained change may be an attack, so do not just accept it.
2. Update the variable and restart the application.

Only one certificate can be pinned at a time, so there is no overlap: expect failures between the
provider's change and your update. To shorten them:

- Alert on `not the pinned certificate` in your logs, and treat it as an incident.
- Track the expiry shown by the command above. An expired certificate still connects, so pemba
  will not warn you, but the provider may replace it around that date.
- Find out what makes your provider replace the certificate, and include the pin in that runbook.

### Railway

These details come from Railway's public [`postgres-ssl`](https://github.com/railwayapp-templates/postgres-ssl)
image, read and run locally on 2026-11-04. They have **not been confirmed on a real Railway service**, and
the image can change, so check your own certificate with the commands above.

- Its certificate has `CA:TRUE`, is issued by a private root, and names `localhost` and the private
  domain. The platform trust store refuses it on any host, including the private domain.
- Without a pin, the connection fails with `invalid peer certificate:
  Other(OtherError(CaUsedAsEndEntity))`.
- It lasts 820 days by default, and is stored on the service's volume, in
  `/var/lib/postgresql/data/certs`.

The image only considers replacing the certificate when the container starts, so a replacement
fails every new connection at once: the restart has already dropped the old ones.

| When the container starts | Certificate | Evidence |
|---|---|---|
| Restart or redeploy, same volume | Kept | Run locally |
| Fewer than 30 days left | **Replaced** | Run locally |
| The volume is new or empty, as in a new environment or service | **A different one** | Run locally |
| The certificate file is missing | **Replaced** | Run locally |
| Restored from a pgBackRest backup, which does not include certificates | **Replaced** | Image source only |
| Major version upgrade | Kept | Image source only |

So the routine case is the 820-day expiry: the first restart or redeploy after about day 790 breaks
the pin, even if nobody meant to change anything. If the service does not restart, the old
certificate stays in place past its expiry, and pinned connections keep working.

Each environment has its own certificate, so give each one its own pin.

To replace it on your own schedule:

1. Note the expiry from the [commands above](#get-the-certificate), and set a reminder for 30 days
   before it. The image does not renew any earlier.
2. Inside those 30 days, in a quiet period, restart the database service. New connections fail
   from here until step 4 is done.
3. Read the new certificate from a shell on the database service:

   ```sh
   openssl x509 -in /var/lib/postgresql/data/certs/server.crt
   ```

4. Put the output in the application's variable, such as `PG_SERVER_CERT`, and redeploy the
   application.

After an unplanned replacement, follow steps 3 and 4, but first work out why it changed. If none of
the causes in the table applies, treat it as a possible attack.
