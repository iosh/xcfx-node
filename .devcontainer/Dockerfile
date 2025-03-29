FROM ubuntu:22.04


ARG USERNAME=user-name-goes-here
ARG USER_UID=1000
ARG USER_GID=$USER_UID

# Create the user
RUN groupadd --gid $USER_GID $USERNAME \
     && useradd --uid $USER_UID --gid $USER_GID -m $USERNAME \
     #
     # [Optional] Add sudo support. Omit if you don't need to install software after connecting.
     && apt-get update \
     && apt-get install -y sudo \
     && echo $USERNAME ALL=\(root\) NOPASSWD:ALL > /etc/sudoers.d/$USERNAME \
     && chmod 0440 /etc/sudoers.d/$USERNAME

RUN  sudo apt-get update && sudo apt-get install curl lsb-release wget software-properties-common gnupg -y     
RUN wget https://apt.llvm.org/llvm.sh && chmod u+x llvm.sh && ./llvm.sh 18
RUN sudo apt-get install -y libsqlite3-dev pkg-config libssl-dev cmake libc++-18-dev git curl unzip

USER ${USERNAME}

WORKDIR /home/${USERNAME}

RUN  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | bash -s -- -y && \ 
     . "$HOME/.cargo/env"

RUN git clone  --depth 1  --single-branch --branch v0.40.1  https://github.com/nvm-sh/nvm.git .nvm  && \
     echo 'export NVM_DIR="$HOME/.nvm"' >> $HOME/.bashrc && \
     echo '[ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"' >> $HOME/.bashrc && \
     echo '[ -s "$NVM_DIR/bash_completion" ] && \. "$NVM_DIR/bash_completion"' >> $HOME/.bashrc && \
     . $HOME/.bashrc && \
     export NVM_DIR="$HOME/.nvm" &&  [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh" && \
     nvm install 20 && npm install -g pnpm



ENV CC=clang-18
ENV CXX=clang++-18
ENV CXXFLAGS="-std=c++11 -stdlib=libc++"
ENV LDFLAGS="-stdlib=libc++"

# ENV OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu
# ENV OPENSSL_INCLUDE_DIR=/usr/include/openssl
# ENV OPENSSL_STATIC=yes