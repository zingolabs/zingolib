The following is required to successfully update the builder image for Github CI workflows:
 - copy lightwalletd binary into the zingolib/docker-ci/ directory
 - run 'docker build -t zingodevops/ci-build:latest .' to build the image locally
 - run 'docker login' and fill in the credentials for DockerHub
 - run 'docker push zingodevops:ci-build:latest' to push to DockerHub
