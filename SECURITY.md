# Security Policy

## Reporting a vulnerability

Please report privately, not in a public issue.

- **Preferred:** GitHub's private vulnerability reporting. Open the repository's
  **Security** tab and choose **Report a vulnerability**, or go straight to
  <https://github.com/distronode-corporation/district-linux/security/advisories/new>.
- **Fallback:** email **opensource@distronode.com** if you cannot use GitHub.

Include what you did, what happened, and what you expected. A proof of concept is
welcome but not required. Never include a real access or refresh token, and never
include another person's data (a call transcript, a phone number, a contact); if one
is part of the problem, say where it appeared, not what it was.

Expect an acknowledgement within a few working days. There is no paid bug bounty;
what you get is credit in the changelog entry for the fix, if you want it.

## Supported versions

Only the latest release is supported. Fixes go into a new release rather than being
backported.

| Version | Supported |
| --- | --- |
| Latest 0.x release, until 1.0 | Yes |
| Latest 1.x release, from 1.0 | Yes |
| Anything older | No |

The project is pre-release and has no releases yet.

## Security model

The design the app is being built to, so a report can say which part of it breaks.
The project is pre-release, and parts of this are not implemented yet.

### Sign-in

- Sign-in is OAuth 2.0 authorization code with PKCE, in the system browser. The app
  never shows a password field and never sees your password.
- The browser returns to the app through the custom URI scheme `districtai://auth`.
  Another application on the same machine can register the same scheme and receive
  the authorization code. PKCE neutralises that: the code is useless without the
  verifier, and the verifier never leaves the app.

### Tokens

- The refresh token is kept in the desktop secret store (the Secret Service, or
  inside a Flatpak an encrypted keyring file whose key comes from the secret
  portal). It is never written to a plain file, a log or a settings store.
- The access token is kept only in memory.
- Signing out removes the refresh token from the secret store and forgets the
  access token.

### Requests

- Every API call carries the access token as a bearer header and names its
  workspace explicitly. There is no cookie store.
- HTTP redirects are not followed, so the token never travels to a host the app did
  not choose. TLS is rustls with certificate verification and no option to turn it
  off.

### Privileges

The app runs as your user and never asks for or needs administrator rights. It
installs no system service and no privileged helper.

## Scope

In scope, for example:

- A token reaching disk outside the secret store, a log line, an error message or a
  crash report.
- A token being sent to a host other than the District AI service.
- Completing sign-in with an authorization code the app did not request.
- One workspace's data being shown or sent in the context of another.

Out of scope:

- The desktop secret store's own security model, and the fact that anything
  running as your user can ask it for your items.
- Vulnerabilities in the District AI service itself rather than in this client.
  Report those privately to the contact published in
  <https://www.distronode.com/.well-known/security.txt>.
