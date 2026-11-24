# Submitting Modules, Publishing Catalogs & Lifecycle Management

> [!IMPORTANT]
> ## Keyglass is an independent project, not affiliated with or endorsed by Flux.

Keyglass uses a **zero-server, Git-native module ecosystem** modeled after Obsidian Community Plugins and Homebrew Taps. Authors own their code in their own GitHub repositories, and Keyglass Studio discovers, validates, updates, and migrates modules using static `catalog.json` files.

---

## 1. Preparing Your Module for Distribution

Before sharing a module, make sure its `module.toml` includes clear metadata and passes validation:

```toml
id = "widget-my-tool"
name = "My Tool"
version = "1.0.0"
description = "One-line summary shown in Manage Modules and Browse Registries."
author = "Your Name"
license = "MIT OR Apache-2.0"
homepage = "https://github.com/you/keyglass-my-tool"
repository = "https://github.com/you/keyglass-my-tool"
support_url = "https://github.com/you/keyglass-my-tool/issues"
discord_url = "https://discord.gg/your-invite"
changelog = "v1.0.0: Initial release."
category = "widget"
tags = ["top-bar", "productivity"]
stages = ["tile"]
host_requirement = "none" # or "optional" / "required" if using [entrypoints].host
```

Run the local validator and profiler before publishing:

```bash
fluxctl modules validate ~/.local/share/openflux/v2/modules/local/widget-my-tool
fluxctl perf --watch   # Check your module's per-frame Lua execution time (µs)
```

---

## 2. Submitting Your Module to the Default Official Registry

The default catalog shown in **Keyglass Studio → Manage Modules → Browse Registries** is built from [`keyglass-dev/keyglass-registry`](https://github.com/keyglass-dev/keyglass-registry).

### How to Submit
1. Host your module in a public GitHub repository (either at the repo root or a subdirectory like `my-module/`).
2. Include a clear `LICENSE`, `README.md`, `module.toml`, and (if your module runs a `[entrypoints].host` script) explain clearly what the host script accesses.
3. Fork [`keyglass-dev/keyglass-registry`](https://github.com/keyglass-dev/keyglass-registry) and add one entry to `sources.json`:
   ```json
   { "source": "github:your-user/your-repo" }
   ```
   *(For a monorepo subdirectory, use `"github:your-user/your-repo//subdir"`.)*
4. Open a Pull Request. The repository’s CI validates your `module.toml`, checks runtime contracts, computes the SHA-256 content hash, and surfaces permission disclosure (`Sandboxed Lua` vs. `Host Helper`) on the PR.
5. Once merged, GitHub Actions rebuilds `catalog.json` and publishes it to GitHub Pages. Users immediately see your module in **Browse Registries** and get automatic update detection whenever you bump `version` (or change files) and update `changelog`.

---

## 3. Publishing Your Own Custom Module List (“Tap”)

You never need permission from `keyglass-dev` to distribute modules! Anyone can host their own independent catalog on GitHub Pages for $0/month:

1. Create a public repo containing a `sources.json` and a copy of `scripts/build-catalog.py` (from `keyglass-dev/keyglass-registry`).
2. Run `python3 scripts/build-catalog.py` (or enable the included `.github/workflows/build-catalog.yml` workflow) to publish a static `catalog.json`.
3. Users subscribe to your catalog in **Keyglass Studio → Manage Modules → Lists & Direct Install → + Add List** by pasting your `https://<you>.github.io/<repo>/catalog.json` URL.
4. Modules from your catalog appear in **Browse Registries** (filterable by list) and receive automatic `vX → vY` update checks with changelogs.

---

## 4. Requesting Your Catalog Be Included by Default

If you maintain a high-quality, community-trusted collection of Keyglass modules (for example, a dedicated sim-racing, DAW/audio, or streaming collection) and would like to propose including your catalog in Keyglass’s default subscribed lists:

### Review Criteria
- **Active maintenance & clear ownership:** Public repository with an issue tracker (`support_url`), clear maintainer(s), and an OSI-approved license for every listed module.
- **Automated validation:** Your catalog repository runs automated `catalog.json` builds with SHA-256 hashes and rejects malformed manifests.
- **Transparent permissions:** Every module that runs host code (`host_requirement != "none"`) documents why and keeps host scripts readable/auditable (no obfuscated binaries).
- **Non-conflicting IDs:** Module IDs are namespaced cleanly (e.g. `sim-tachometer` rather than colliding with core IDs like `widget-clock`).

### How to Request
Open an issue or Pull Request on [`keyglass-dev/keyglass`](https://github.com/keyglass-dev/keyglass) titled **`Catalog Proposal: <Your Catalog Name>`** containing:
- The static `catalog.json` URL and source repository link
- Maintainer contact / support & Discord links
- A brief summary of the modules included and your review policy for updates

---

## 5. Direct Installs (No Catalog Required)

Users can install any module directly without any catalog by pasting `github:owner/repo`, `github:owner/repo//subdir`, an archive URL, or a local folder path under **Manage Modules → Lists & Direct Install**.

- Keyglass records the exact origin in `.openflux-source.json` and checks it automatically on **Check for Updates**.
- If the user uninstalls a directly-installed module, Keyglass remembers its source under **Previously Removed Modules** so it can be re-installed in one click later (or forgotten with **Forget**).

---

## 6. Deprecating, Renaming, or Moving a Module

If you ever rename your module, move it to a new repository/organization, or retire it in favor of a replacement, leave a lightweight tombstone in the old module’s `module.toml` (and rebuild the catalog if listed):

```toml
version = "1.2.1"
deprecated = true
deprecation_message = "Moved to the new community repo with improved performance. Click Migrate to keep your settings."
moved_to = "github:new-org/new-repo"      # New upstream source URL/path
replaced_by = "new-module-id"             # Optional: include if the module ID changed
```

### What Happens for Existing Users
- **Never silently redirects:** For security, Keyglass never switches repositories without user consent.
- **Clear banner & 1-click Migrate:** Existing users see an amber **Moved / Replaced** banner with your `deprecation_message` and a **Migrate** button. Clicking **Migrate** runs normal TOCTOU-free permission disclosure for the new location, copies their saved settings and top-bar slot placement to the new module ID, and removes the old tombstoned copy.
- **Deprecated without replacement:** Set `deprecated = true` and `deprecation_message = "..."` (without `moved_to`). Existing users see a **Deprecated** notice, and new users won’t see it in default **Browse Registries** results unless they search for it explicitly.
