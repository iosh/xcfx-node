FROM ubuntu:24.04
RUN apt-get update && apt-get install clang -y 

RUN apt-get install -y libsqlite3-dev pkg-config libssl-dev cmake libc++-dev git curl unzip

RUN  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && \ 
     . ~/.bashrc  && \
     rustup target add x86_64-unknown-linux-gnu

RUN curl -fsSL https://fnm.vercel.app/install | bash && \
     . ~/.bashrc && \
     fnm use --install-if-missing 20 && \
     npm install pnpm -g 





ENV PATH="/root/.cargo/bin:${PATH}"
ENV CC=clang
ENV CXX=clang++
ENV CXXFLAGS="-std=c++11 -stdlib=libc++"
ENV LDFLAGS="-stdlib=libc++"