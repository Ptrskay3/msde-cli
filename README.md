# MSDE-CLI

### Local development

For local development, you may start the local auth server in a different terminal:

```sh
RUST_LOG=msde_cli=debug,tower_http=debug cargo r -F local_auth -- run-auth-server
```
This handles the authentication part locally, so the real central service can be avoided.

### Release checklist

Before making a new release, ensure that:
- If you change __ANY__ of the `struct`s or `enum`s that derives `serde::(De)/Serialize`, ensure that it's backward compatible or you provide a migration
 mechanism, so people using older versions don't need to wipe out their existing configs.
- Fill out the package upgrade matrix (only if you changed anything the `package` folder, and it needs special care instead of a simple override).
- Bump the version in `Cargo.toml`
- Check `MERIGO_UPSTREAM_VERSION` in `.cargo/config.toml` and bump it if necessary.

### Environment variables

This is a list of existing environment variables that alter the behavior of the CLI tool.

`MERIGO_AUTH_URL`: Connect to this url for authentication. Useful for local development to override the production URL in builds. The local server is at `http://localhost:8765`.

`MERIGO_UPSTREAM_VERSION`: The current upstream version of the siab_app when this tool was built. This is a compile-time variable.

`MERIGO_TOKEN`: The token used for authentication. Currently only used for the `login` command, but all subcommands will accept this in the future. It'll take precedence over the stored `~/.msde/auth.json` file.

`MERIGO_DEV_PACKAGE_DIR`: The folder where the project is initialized. Useful if you have multiple project locations. Takes precedence over `~/.msde/config.json`.

`MERIGO_NOWARN_INIT`: If you have no project initialized, the tool prints a warning by default. Set this variable to a non-empty string to disable printing that warning. 

### Requires
  - docker compose >=2.20

### TODO:
  - Deprecate the old `credentials.json` when the central service is ready.
  - `status` subcommand to summarize the current state of the configuration and system
  - To embed the commit sha, it's the best to use `https://crates.io/crates/vergen` probably.
  - Preserve the stages.yml file on upgrade.
  - Maybe provide a "run-consistency-checks" function to scan the games directory, and check whether it follows our rules, like
    - stages.yml points to existing and valid entries
    - local_config.yml is consistent with the directory structure (guid is the same within games)
    - if MSDE is running, the live config doesn't contain any non-existing stuff in the project dir
  - A lot of TODO comments inside the code
