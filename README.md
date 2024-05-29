# Requires
  - docker compose >=2.20

## TODO:
  - Detect if we have the images locally, and adjust timeout value based on that (cold start)
    - There's a problem with kibana health check on cold starts
  - `status` subcommand to summarize the current state of the configuration and system
  - To embed the commit sha, it's the best to use `https://crates.io/crates/vergen` probably.
  - A lot of TODO comments inside the code

