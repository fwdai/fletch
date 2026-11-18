#!/bin/sh
# Push a cloud image build-docker.sh built, once. Its tag,
# <box image version>-<REVISION>, names one image: a tag the registry has is
# never pushed again, so a change to what the image contains needs a REVISION
# bump (or a new box image version) to reach the registry. Needs `docker
# login` to the registry (ghcr.io: the workflow's token, `packages: write`).
#
#   publish.sh IMAGE     e.g. ghcr.io/fwdai/fletch-cloud:0.5.2-1 (OUT_DIR/image)
set -eu

IMAGE=${1:?usage: publish.sh IMAGE}
docker image inspect "$IMAGE" >/dev/null ||
  { echo "publish.sh: no image $IMAGE here (build-docker.sh first)" >&2; exit 1; }
# Pushed only when the registry says it does not know the tag: an answer it
# cannot give (a network error, a refusal) fails rather than risk pushing
# over an image that is there.
if out=$(docker manifest inspect "$IMAGE" 2>&1); then
  echo "$IMAGE is in the registry; nothing to push"
  exit 0
fi
case "$out" in
  *"no such manifest"*) ;;
  *) echo "::error::cannot tell whether the registry has $IMAGE: $out"; exit 1 ;;
esac
docker push "$IMAGE"
echo "pushed $IMAGE"
