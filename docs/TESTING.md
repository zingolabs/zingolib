archlinux:

cargo install cargo-nextest

cd ~/gits
git clone https://github.com/zcash/zcash.git 
 sudo apt-get install \
 build-essential pkg-config libc6-dev m4 g++-multilib \
 autoconf libtool ncurses-dev unzip git python3 python3-zmq \
 zlib1g-dev curl bsdmainutils automake libtinfo5
paru -S
  build-essential -> base-devel
  libc6-dev -> glibc
  g++-multilib (supposedly in gcc)
  ncurses-dev -> ncurses
  python3-zmq
  zlib1g-dev
  libtinfo5

  libc++abi
  
./zcutil/build.sh -j$(nproc)

