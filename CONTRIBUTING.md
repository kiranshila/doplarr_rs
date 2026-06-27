# Contributing to Doplarr

All help is welcome and greatly appreciated! If you would like to contribute to the project, the following instructions should get you started...

## AI Assistance

Using AI tools to help you contribute is fine. We just ask that you stay in the
driver's seat: **AI-assisted development, yes; fully AI-driven contributions,
no.** Use AI as a tool to help you write code, not as an agent that
autonomously generates an entire contribution and submits it on your behalf.

A few simple expectations:

- **Understand your code.** You should be able to explain and answer questions
  about anything you submit.
- **Write the PR description in your own words.** A short, clear explanation of
  *your* change is far more useful than a pasted AI summary.
- **Test your change** before opening the PR (see [Contributing Code](#contributing-code)).
- **Keep it focused.** A PR that claims to fix one thing but touches a lot of
  unrelated code is hard to review — broad AI prompts tend to cause this.
- **Disclose AI assistance** in the PR description, along with roughly how much
  was used (e.g. docs only vs. code generation). Trivial tab-completion of
  single keywords or short phrases doesn't need to be disclosed.

Example disclosures:

> **AI Disclosure:** This PR was written primarily by Claude Code.
> **AI Disclosure:** I consulted ChatGPT to understand the codebase, but the solution was authored manually.
> **AI Disclosure:** None.

Disclosure isn't about discouraging AI use — it just helps reviewers know how
much scrutiny a change needs, and it's a courtesy to the humans on the other
end of the pull request.

## Development

### Tools Required

- [Git](https://git-scm.com/downloads)
- A Rust toolchain. The pinned channel and components live in
  [`rust-toolchain.toml`](rust-toolchain.toml) (currently Rust 1.96, edition
  2024) and `rustup` will install them automatically.
- [Nix](https://nixos.org/download/) **(recommended)**. The flake provides a
  complete development shell with the correct toolchain and tools (including
  `openapi-generator-cli`, used to regenerate the `*_api` bindings). Enter it
  with `nix develop`, or let [direnv](https://direnv.net/) load it
  automatically via the checked-in `.envrc`.

### Getting Started

1. [Fork](https://help.github.com/articles/fork-a-repo/) the repository to your own GitHub account and [clone](https://help.github.com/articles/cloning-a-repository/) it to your local device:

   ```bash
   git clone https://github.com/YOUR_USERNAME/doplarr_rs.git
   cd doplarr_rs/
   ```

2. Add the remote `upstream`:

   ```bash
   git remote add upstream https://github.com/activexray/doplarr_rs.git
   ```

3. Create a new branch off `main`:

   ```bash
   git switch -c BRANCH_NAME main
   ```

   - It is recommended to give your branch a meaningful name, relevant to the feature or fix you are working on.
     - Good examples:
       - `docs-config`
       - `feature-jellyseerr-backend`
       - `fix-rootfolder-preset`
     - Bad examples:
       - `bug`
       - `docs`
       - `feature`
       - `fix`
       - `patch`

4. Enter the development environment and build:

   ```bash
   nix develop        # or: direnv allow
   cargo build
   ```

   - Without Nix, `rustup` will install the toolchain pinned in
     `rust-toolchain.toml`, but you will need `openapi-generator-cli` on your
     `PATH` if you intend to regenerate API bindings.

5. Create your patch and test your changes.

   - Keep your fork up to date by rebasing on `upstream`:

     ```bash
     git fetch upstream
     git rebase upstream/main
     git push origin BRANCH_NAME -f
     ```

### Adding a New Backend

See [`README_DEVELOPER.md`](README_DEVELOPER.md) for the full walkthrough on
generating API bindings, implementing the `MediaBackend`/`MediaItem` traits,
and wiring up config and initialization.

### Contributing Code

- If you are taking on an existing bug or feature ticket, please comment on the [issue](/../../issues) to avoid multiple people working on the same thing.
- Pull request titles **must** follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) (e.g. `fix:`, `feat:`, `chore:`, `docs:`). This matches the existing commit history.
- Please make meaningful commits, or squash them prior to opening a pull request.
  - Do not squash commits once people have begun reviewing your changes.
- Always rebase your branch onto the latest `main` branch. It is your responsibility to keep your branch up-to-date.
- You can create a "draft" pull request early to get feedback on your work.
- Your code **must** pass CI. CI runs `nix flake check`, which includes:
  - `cargo fmt` (rustfmt) and `taplo` for TOML formatting
  - `cargo clippy` with **`--deny warnings`**
  - the build and test suite
  - `cargo audit`

  Run these locally before pushing:

  ```bash
  nix flake check
  # or, without Nix:
  cargo fmt --all
  cargo clippy --all-targets -- --deny warnings
  cargo test
  ```

- Open pull requests against `main`.
- If you have questions or need help, reach out via [Discussions](/../../discussions) or our [Discord server](https://discord.gg/890634173751119882).

### User-Facing Text

Doplarr's user-facing surface is the Discord bot. When adding or changing text
that users see:

1. Be concise and clear, and use as few words as possible to make your point. Prefer minimal, low-noise messages.
2. Capitalize proper nouns and product names correctly: Discord, Radarr, Sonarr, Seerr, Plex, etc.
3. Use the appropriate Unicode characters for ellipses, arrows, and other special characters/symbols.
4. Do your best to check for spelling errors and grammatical mistakes.

## Attribution

This contribution guide and our Code of Conduct are adapted from
[Seerr](https://github.com/seerr-team/seerr), whose contribution guide was in
turn inspired by the [Next.js](https://github.com/vercel/next.js),
[Radarr](https://github.com/Radarr/Radarr), and
[Ghostty](https://github.com/ghostty-org/ghostty) contribution guides.
</content>
