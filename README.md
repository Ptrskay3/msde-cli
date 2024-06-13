# MSDE-CLI

### Local development

For local development, you may start the local auth server in a different terminal:

```sh
RUST_LOG=msde_cli=debug,tower_http=debug cargo r -F local_auth -- run-auth-server
```
This handles the authentication part locally, so the real central service can be avoided.


### Requires
  - docker compose >=2.20

### TODO:
  - Deprecate the old `credentials.json` the central service is ready.
  - `status` subcommand to summarize the current state of the configuration and system
  - To embed the commit sha, it's the best to use `https://crates.io/crates/vergen` probably.
  - Preserve the stages.yml file on upgrade.
  - Maybe provide a "run-consistency-checks" function to scan the games directory, and check whether it follows our rules, like
    - stages.yml points to existing and valid entries
    - local_config.yml is consistent with the directory structure (guid is the same within games)
    - if MSDE is running, the live config doesn't contain any non-existing stuff in the project dir
  - A lot of TODO comments inside the code
