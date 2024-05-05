#!/bin/bash

# Create KV bucket types

## By default, if the riak install is shut down, this poststart runs and attempts to re-install datatypes
## the startup bombs.  This version writes a lockfile and will just skip the poststart scripts after the first run
LOCKFILE=/usr/lib/riak/poststart.lck
if test -f "$LOCKFILE"; then
    echo "$LOCKFILE exists, skipping schema poststart"
else
    echo "Looking for datatypes in $SCHEMAS_DIR..."
    for f in $(find $SCHEMAS_DIR -name *.dt -print); do
      BUCKET_NAME=$(basename -s .dt $f)
      BUCKET_DT=$(cat $f)
      $RIAK_ADMIN bucket-type create $BUCKET_NAME "{\"props\":{\"datatype\":\"$BUCKET_DT\"}}"
      $RIAK_ADMIN bucket-type activate $BUCKET_NAME
    done
fi
touch $LOCKFILE
