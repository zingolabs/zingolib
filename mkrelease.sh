#!/bin/bash
# This script depends on a docker image already being built
# To build it, 
# cd docker
# docker build --tag rustbuild:latest .

POSITIONAL=()
while [[ $# -gt 0 ]]
do
key="$1"

case $key in
    -v|--version)
    APP_VERSION="$2"
    shift # past argument
    shift # past value
    ;;
    *)    # unknown option
    POSITIONAL+=("$1") # save it in an array for later
    shift # past argument
    ;;
esac
done
set -- "${POSITIONAL[@]}" # restore positional parameters

if [ -z $APP_VERSION ]; then echo "APP_VERSION is not set"; exit 1; fi

# Write the version file
echo "pub const VERSION: &str = \"$APP_VERSION\";" > bin/cli/src/version.rs

# First, do the tests
cd lib && cargo test --release
retVal=$?
if [ $retVal -ne 0 ]; then
    echo "Error"
    exit $retVal
fi
cd ..

# Compile for mac directly
cargo build --release 

#macOS
codesign -f -s "Apple Distribution: Concision Systems LLC (5N76B7JDDT)" target/release/zingolib-cli --deep
rm -rf target/macOS-zingolib-cli-v$APP_VERSION
mkdir -p target/macOS-zingolib-cli-v$APP_VERSION
cp target/release/zingolib-cli target/macOS-zingolib-cli-v$APP_VERSION/

# Now sign and zip the binaries
# macOS
gpg --batch --output target/macOS-zingolib-cli-v$APP_VERSION/zingolib-cli.sig --detach-sig target/macOS-zingolib-cli-v$APP_VERSION/zingolib-cli 
cd target
cd macOS-zingolib-cli-v$APP_VERSION
gsha256sum zingolib-cli > sha256sum.txt
cd ..
zip -r macOS-zingolib-cli-v$APP_VERSION.zip macOS-zingolib-cli-v$APP_VERSION 
cd ..

# For Windows and Linux, build via docker
docker run --rm -v $(pwd)/:/opt/zingolib rustbuild:latest bash -c "cd /opt/zingolib && cargo build --release && cargo build --release --target armv7-unknown-linux-gnueabihf && cargo build --release --target aarch64-unknown-linux-gnu && SODIUM_LIB_DIR='/opt/libsodium-win64/lib/' cargo build --release --target x86_64-pc-windows-gnu"

#Linux
rm -rf target/linux-zingolib-cli-v$APP_VERSION
mkdir -p target/linux-zingolib-cli-v$APP_VERSION
cp target/release/zingolib-cli target/linux-zingolib-cli-v$APP_VERSION/
gpg --batch --output target/linux-zingolib-cli-v$APP_VERSION/zingolib-cli.sig --detach-sig target/linux-zingolib-cli-v$APP_VERSION/zingolib-cli
cd target
cd linux-zingolib-cli-v$APP_VERSION
gsha256sum zingolib-cli > sha256sum.txt
cd ..
zip -r linux-zingolib-cli-v$APP_VERSION.zip linux-zingolib-cli-v$APP_VERSION 
cd ..


#Windows
rm -rf target/Windows-zingolib-cli-v$APP_VERSION
mkdir -p target/Windows-zingolib-cli-v$APP_VERSION
cp target/x86_64-pc-windows-gnu/release/zingolib-cli.exe target/Windows-zingolib-cli-v$APP_VERSION/
gpg --batch --output target/Windows-zingolib-cli-v$APP_VERSION/zingolib-cli.sig --detach-sig target/Windows-zingolib-cli-v$APP_VERSION/zingolib-cli.exe
cd target
cd Windows-zingolib-cli-v$APP_VERSION
gsha256sum zingolib-cli.exe > sha256sum.txt
cd ..
zip -r Windows-zingolib-cli-v$APP_VERSION.zip Windows-zingolib-cli-v$APP_VERSION 
cd ..


#Armv7
rm -rf target/Armv7-zingolib-cli-v$APP_VERSION
mkdir -p target/Armv7-zingolib-cli-v$APP_VERSION
cp target/armv7-unknown-linux-gnueabihf/release/zingolib-cli target/Armv7-zingolib-cli-v$APP_VERSION/
gpg --batch --output target/Armv7-zingolib-cli-v$APP_VERSION/zingolib-cli.sig --detach-sig target/Armv7-zingolib-cli-v$APP_VERSION/zingolib-cli
cd target
cd Armv7-zingolib-cli-v$APP_VERSION
gsha256sum zingolib-cli > sha256sum.txt
cd ..
zip -r Armv7-zingolib-cli-v$APP_VERSION.zip Armv7-zingolib-cli-v$APP_VERSION 
cd ..


#AARCH64
rm -rf target/aarch64-zingolib-cli-v$APP_VERSION
mkdir -p target/aarch64-zingolib-cli-v$APP_VERSION
cp target/aarch64-unknown-linux-gnu/release/zingolib-cli target/aarch64-zingolib-cli-v$APP_VERSION/
gpg --batch --output target/aarch64-zingolib-cli-v$APP_VERSION/zingolib-cli.sig --detach-sig target/aarch64-zingolib-cli-v$APP_VERSION/zingolib-cli
cd target
cd aarch64-zingolib-cli-v$APP_VERSION
gsha256sum zingolib-cli > sha256sum.txt
cd ..
zip -r aarch64-zingolib-cli-v$APP_VERSION.zip aarch64-zingolib-cli-v$APP_VERSION 
cd ..
