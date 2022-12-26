# Use latest stable debian release
FROM debian:latest

# Create user
ARG USERNAME=appuser
ARG USER_UID=1000
ARG USER_GID=$USER_UID
RUN groupadd --gid $USER_GID $USERNAME  && \
    useradd --uid $USER_UID --gid $USER_GID -m $USERNAME && \
    apt update && \
    apt install sudo -y && \
    echo $USERNAME ALL=\(root\) NOPASSWD:ALL > /etc/sudoers.d/$USERNAME && \
    chmod 0440 /etc/sudoers.d/$USERNAME
USER $USERNAME

# Install packages
RUN sudo apt install curl build-essential gcc protobuf-compiler libssl-dev pkg-config git apt-transport-https wget gnupg2 unzip -y
    
# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | bash -s -- -y
ENV PATH="/home/${USERNAME}/.cargo/bin:${PATH}"

# Install Zcash and fetch zero-knowledge parameters
RUN wget -qO - https://apt.z.cash/zcash.asc | gpg --import && \
    gpg --export 3FE63B67F85EA808DE9B880E6DEF3BAF272766C0 | sudo apt-key add - && \
    echo "deb [arch=amd64] https://apt.z.cash/ bullseye main" | sudo tee /etc/apt/sources.list.d/zcash.list && \
    sudo apt update && sudo apt install zcash && zcash-fetch-params

# Import lightwalletd binary
COPY /lightwalletd /usr/bin/

# Create source directory
RUN mkdir /home/$USERNAME/src
WORKDIR home/$USERNAME/src

# test only
RUN sudo apt install netcat nano -y
