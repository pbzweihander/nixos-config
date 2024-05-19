# local bin
if test -d ~/.local/bin
    fish_add_path $HOME/.local/bin
end

# mise
if type -q mise
    set -lxp fish_user_paths ~/.local/share/mise/installs/usage/latest/bin
    mise activate fish | source
    mise completion fish | source
    mise use -g usage >/dev/null
end
