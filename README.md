# Requires:
  - Linux
    - glibc >=2.31
    - openssl
        - ```
          wget http://archive.ubuntu.com/ubuntu/pool/main/o/openssl/libssl1.1_1.1.0g-2ubuntu4_amd64.deb &&
          sudo dpkg -i libssl1.1_1.1.0g-2ubuntu4_amd64.deb
          ```
    - docker compose >=2.20

## TODO:
  - Don't use BufReader with serde_json::from_reader, because it's slower
    https://docs.rs/serde_json/latest/serde_json/fn.from_reader.html
    (On the other hand, it probably doesn't matter because all JSON files are tiny..)
  - `status` subcommand to summarize the current state of the configuration and system
  - To embed the commit sha, it's the best to use `https://crates.io/crates/vergen` probably.
