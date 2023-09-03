The following is required to successfully update the docker container for Github CI workflows:
 - update the lightwalletd and zcash git tags to the latest versions in 'docker-ci/Dockerfile' 
 - change to the `docker-ci` directory
 - run 'docker build -t zingodevops/ci-build:<new image version number> .' to build the image locally 
 - run 'docker login' and fill in the credentials for DockerHub
 - run 'docker push zingodevops/ci-build:<new image version number>' to push to DockerHub
 - update github workflow files to the new image version number