# mise
if type -q mise
    set -lxp fish_user_paths ~/.local/share/mise/installs/usage/latest/bin
    mise activate fish | source
    mise completion fish | source
end
