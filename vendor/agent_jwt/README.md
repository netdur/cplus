# agent_jwt

HS256 **JWT verification** for an agent surface's authorization policy.

`agent_core::auth` hands your policy an opaque `token: str` and takes back a
`Grant`. This turns one common kind of token into something a policy can decide
on. The core deliberately does not do this itself — verification needs crypto
and a clock, and a core that reaches for either stops being unit-testable and
stops porting.

```toml
[dependencies]
agent_jwt = "*"
```

```cplus
import "agent_jwt/agent_jwt" as jwt;
import "agent_core/auth" as auth;

fn my_policy(req: auth::Request) -> auth::Grant {
    let v: jwt::Verified = jwt::verify_hs256(req.token, SECRET, 60 as i64);
    return match v {
        jwt::Verified::Valid(c) => {
            if { c.scope.view() } == "autofill" { auth::protected_operator() }
            else { auth::operator() }
        }
        jwt::Verified::Invalid(_) => auth::nothing(),
    };
}
```

Bind the result to a **named local** before matching it, as above. `Verified`
owns Texts, and a by-value parameter in C+ borrows rather than consumes.

## What it refuses

Every one is a published break of some JWT library:

| Refused | Because |
|---|---|
| `alg: "none"` | the header is attacker-supplied; a verifier that trusts it accepts an empty signature |
| any non-HS256 `alg` | algorithm confusion — the algorithm is not a parameter here |
| a wrong or truncated MAC | compared **constant time**, length-checked first |
| a tampered payload | the signature covers the **encoded** segments, byte for byte as they arrived |
| a malformed segment | `stdlib/base64` refuses unknown characters instead of skipping them |
| `exp` past / `nbf` future | checked **after** the signature, so a forgery never learns its MAC was fine |

`leeway_secs` widens the expiry window both ways for clock skew; 60 is the usual
choice.

## What it does not do

No RS256/ES256, no JWKS fetching, no issuer/audience policy. `iss` and `aud` are
parsed and handed over — deciding what they mean is the application's job, as is
deciding what a scope is worth.

## Claims

`sub`, `iss`, `aud`, `scope`, `exp`, `nbf`, `iat`, plus `has_exp` / `has_nbf`
(0 is a real time) and `payload_json` — the verified payload as JSON source, for
a claim this struct does not name.

## Signing

`sign_hs256(payload_json, secret)` exists so the verifier's tests can assert the
success path, and because an app minting its own short-lived agent tokens needs
exactly this and nothing more.

## Tests

```
cd vendor/agent_jwt && cpc test
```
