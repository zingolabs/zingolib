The container test image is versioned from `.env.testing-artifacts`,
`rust-toolchain.toml`, and this `docker-ci` directory.

For local reproducible test runs:
 - update `.env.testing-artifacts` if zebra or the Zaino image tag changes,
   always together with the tag's manifest digest (the file is the single
   source of truth; the Dockerfile ARGs carry no defaults)
 - run `makers build-image` to build the local image
 - run `makers run` to execute tests inside the image

For Github CI workflow images:
 - run `makers compute-image-tag` to get the reproducible image tag
 - run `docker login ghcr.io` with a GitHub token that holds write:packages
 - run `docker push ghcr.io/zingolabs/ci-build:<computed image tag>` to publish it
 - update github workflow files to the new image tag

 NOTE: if `sudo` is necessary use `sudo` with all commands including login.
 for MAC M1...5 -> 'docker buildx build --platform=linux/amd64 -t ghcr.io/zingolabs/ci-build:<computed image tag> --load .'
