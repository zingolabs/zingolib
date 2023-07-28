The following is required to successfully update the builder image for CI:
 - change to `docker-ci` directory
 - run 'docker build -t zingodevops/ci-build:<image version number> .' to build the image locally
 - run 'docker login' and fill in the credentials for DockerHub
 - run 'docker push zingodevops/ci-build:<image version number>' to push to DockerHub
 - update github workflow files to new version number
