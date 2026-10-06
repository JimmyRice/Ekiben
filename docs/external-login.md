# External sign-in: Sign in with Apple, WeChat, QQ, …

Besides email and password, people can sign in to Kippu through providers they already use.
Kippu itself ships no client for any provider: each one has its own flow, keys and rules, and
deployments need different ones. Instead it has two building blocks, and you write a small
module that connects them to the provider of your choice:

```text
app --> your module --> provider (Apple, WeChat, ...) --> your module
                                                          |  knows who it is
                       external::link_or_create <---------+
                       sessions::issue --> ordinary Kippu access + refresh tokens
```

After that the person is an ordinary Kippu user. Every API call uses Kippu's own tokens, so
sessions can be revoked, roles and organizations work as usual, and nothing depends on the
provider being reachable.

## What Kippu provides

- **`kippu_core::auth::external::link_or_create(&state, identity)`** resolves an
  `ExternalIdentity` (provider name, the provider's subject, optionally email and display
  name) to an account:
  1. an identity linked before signs in to its account;
  2. otherwise, if `auth.link_by_verified_email` is on and the provider verified the email,
     the identity is linked to the account with that email;
  3. otherwise a new `user` account is created — with no password, and with the email only if
     the provider verified it. WeChat and QQ usually share no email at all, which is fine.

  Concurrent first sign-ins of one person create one account. Every link is written to the
  audit log.
- **`kippu_core::auth::external::link(&state, &account, &identity)`** links a provider to an
  account that is already signed in, e.g. from its settings page.
- **`kippu_core::auth::sessions::issue(&state, account)`** signs an account in: an access
  token plus a single-use refresh token, the same response as `POST /v1/auth/login`.

People manage their sign-in methods themselves:

| Endpoint | |
|---|---|
| `GET /v1/me/identities` | The providers linked to the account. |
| `DELETE /v1/me/identities/{provider}/{subject}` | Unlink one. The last way to sign in cannot be removed (`409 last-sign-in-method`) until a password is set. |
| `PUT /v1/me/email` | Add or change the email, e.g. after signing up through WeChat. |
| `PUT /v1/me/password` | Set a first password (no current one needed), or change it (`current_password` required). |

## Writing a provider module

A module adds routes like any other (see `kippu_core::module`). The usual shape is a pair of
routes per provider:

- `GET /v1/auth/<provider>/start` creates a random `state` (against CSRF) and, if the provider
  supports it, a PKCE verifier; stores both on the client side (an `HttpOnly` cookie, or
  hands them to the app); redirects to the provider.
- `GET /v1/auth/<provider>/callback` checks that `state` matches, exchanges the code with the
  provider (sending the PKCE verifier and your client secret), **verifies what the provider
  returns** — the signature of an ID token, or the response of the provider's HTTPS API — and
  then calls `link_or_create` and `sessions::issue`.

Native apps often do the provider's part themselves (the Sign in with Apple sheet, the WeChat
SDK) and send the result to a single `POST /v1/auth/<provider>` route instead; the module
then verifies that result on the server before trusting it.

```rust,ignore
let identity = ExternalIdentity::new("apple", claims.sub)
    .email(claims.email, claims.email_verified)
    .display_name(name);
let linked = external::link_or_create(&state, identity).await?;
Ok(Json(sessions::issue(&state, linked.account).await?))
```

Keep the module stateless like the rest of Kippu: anything that must survive from `start` to
`callback` travels with the client (a cookie, the app) or is signed.

The `external_login` example in `crates/kippu/server/examples` is a complete module with a
pretend provider that approves everyone, so it runs without registering anywhere:

```bash
cargo run -p kippu-server --example external_login -- serve --config kippu.toml
```

Open `http://localhost:8080/v1/auth/pretend/start?name=Miku` in a browser: it goes through
the pretend provider and comes back with a Kippu session. Signing in again with the same name
returns the same account.

### Notes on common providers

- **Sign in with Apple**: verify the `id_token` (ES256) against Apple's published keys, and
  check `iss`, `aud` (your client id) and `exp`. The subject is `sub`. Apple may hide the real
  address behind a relay address and only sends the name on the very first sign-in.
- **WeChat (微信)**: exchange the `code` for an access token server-side with your app secret;
  the response carries `openid` and, for apps under one open-platform account, `unionid`. Use
  `unionid` as the subject when you have it, so the same person is recognized across your
  apps. No email is provided.
- **QQ**: similar to WeChat; the `openid` (or `unionid`, if enabled) is the subject.

## Security checklist

- The **subject** must be the provider's stable, unique identifier — never an email or a
  display name, which can change or be reused.
- Always check **`state`**; use **PKCE** where the provider supports it.
- Trust only what the server verified with the provider, never claims the client sends.
- Pick a provider name once and keep it: `(provider, subject)` is the identity's key.
- Leave **`auth.link_by_verified_email`** off unless every account's email is known to be
  verified. Kippu does not verify the emails people register with, so with it on, whoever
  registered an address first would gain the provider's sign-in for it.
