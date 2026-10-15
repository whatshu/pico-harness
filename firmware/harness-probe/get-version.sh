#!/bin/sh
if command -v git >/dev/null 2>&1
then
    VERSION=$(git describe --always --tags --dirty 2>/dev/null || echo unknown)
else
    VERSION="unknown"
fi

if [ ! -e generated/probe ]
then
    mkdir -p generated/probe
fi

cat > generated/probe/version.h << EOF
#ifndef _PROBE_VERSION_H
#define _PROBE_VERSION_H

#define PROBE_VERSION "${VERSION}"

#endif
EOF
