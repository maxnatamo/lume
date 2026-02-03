FROM rust:1.93

ARG UID=1000
ARG GID=1000
ARG USERNAME=max

RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get -y install curl pkg-config llvm clang gdb lldb zsh starship

RUN groupadd -g $GID $USERNAME && \
    useradd $USERNAME \
    --create-home \
    --uid $UID \
    --gid $GID \
    --shell /usr/bin/zsh

USER $USERNAME
WORKDIR /usr/src/app

# Install Oh My Zsh
RUN sh -c "$(curl -fsSL https://raw.githubusercontent.com/ohmyzsh/ohmyzsh/master/tools/install.sh)"

# Setup Starship for Zsh
RUN echo 'eval "$(starship init zsh)"' >> /home/$USERNAME/.zshrc

ENTRYPOINT ["/usr/bin/zsh"]
