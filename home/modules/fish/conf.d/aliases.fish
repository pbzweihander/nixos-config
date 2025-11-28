abbr -a rm "rm -i"
abbr -a rmf "rm -f"
abbr -a rmr "rm -ir"
abbr -a rmrf "rm -rf"
abbr -a cp "cp -i"
abbr -a cpr "cp -ir"
abbr -a cpf "cp -f"
abbr -a cprf "cp -rf"
abbr -a mv "mv -i"
abbr -a mvf "mv -f"

# color=auto
abbr -a grep "grep --color=auto"
abbr -a fgrep "fgrep --color=auto"
abbr -a egrep "egrep --color=auto"
abbr -a diff "diff --color=auto"

# change directory
abbr -a ccd "cd .."
abbr -a cccd "cd ../.."
abbr -a ccccd "cd ../../.."
abbr -a cccccd "cd ../../../.."
abbr -a cdd "cd -"
abbr -a ... ../..
abbr -a .... ../../..
abbr -a ..... ../../../..
abbr -a ...... ../../../../..

# lsd
abbr -a l "lsd --group-dirs first --"
abbr -a ls "lsd --group-dirs first --classify --"
abbr -a la "lsd --group-dirs first --classify --almost-all --"
abbr -a ll "lsd --group-dirs first --classify --long --"
abbr -a lla "lsd --group-dirs first --classify --long --almost-all --"
abbr -a lt "lsd --group-dirs first --classify --tree --"
abbr -a lta "lsd --group-dirs first --classify --tree --almost-all --"
# fzf.fish
set fzf_preview_dir_cmd lsd --color always --icon always --almost-all --oneline --classify

# vim
abbr -a vi vim
abbr -a svi "sudo --preserve-env vim"

# helix
abbr -a shx "sudo --preserve-env hx"
abbr -a h hx
abbr -a h. "hx ."

# systemctl
abbr -a sctl "sudo systemctl"
abbr -a usctl "systemctl --user"
abbr -a jctl "sudo journalctl"
abbr -a jctle "sudo journalctl --pager-end"
abbr -a jctleb "sudo journalctl --pager-end --boot=0"
abbr -a jctlebf "sudo journalctl --pager-end --boot=0 --follow"
abbr -a jctlu "sudo journalctl --unit"
abbr -a jctlue "sudo journalctl --pager-end --unit"
abbr -a jctlueb "sudo journalctl --pager-end --boot=0 --unit"
abbr -a jctluebf "sudo journalctl --pager-end --boot=0 --follow --unit"
abbr -a ujctl "journalctl --user"
abbr -a ujctle "journalctl --user --pager-end"
abbr -a ujctleb "journalctl --user --pager-end --boot=0"
abbr -a ujctlebf "journalctl --user --pager-end --boot=0 --follow"
abbr -a ujctlu "journalctl --user --user-unit"
abbr -a ujctlue "journalctl --user --pager-end --user-unit"
abbr -a ujctlueb "journalctl --user --pager-end --boot=0 --user-unit"
abbr -a ujctluebf "journalctl --user --pager-end --boot=0 --follow --user-unit"

# podman
abbr -a pm podman
abbr -a pc podman-compose
abbr -a dk podman
abbr -a dc podman-compose

# clipboard
abbr -a xc "wl-copy --type text/plain"
abbr -a xcn "wl-copy --type text/plain --trim-newline"
abbr -a xp "wl-paste --type text/plain"
abbr -a xpn "wl-paste --type text/plain --trim-newline"

# python
abbr -a py python

# kubernetes
abbr -a ktx kubectx
abbr -a ktc "kubectx -c"
abbr -a kns kubens

# aws
abbr -a pap AWS_PROFILE=personal
abbr -a paws "AWS_PROFILE=personal aws"
abbr -a wap AWS_PROFILE=work
abbr -a waws "AWS_PROFILE=work aws"

# terraform
abbr -a tf terraform
abbr -a ptf "AWS_PROFILE=personal terraform"
abbr -a wtf "AWS_PROFILE=work terraform"
abbr -a ctf "AWS_PROFILE=cftf terraform"

# zoxide
abbr -a zz "z -"

# nix
abbr -a nrs "sudo nixos-rebuild switch"
abbr -a nrb "sudo nixos-rebuild boot"

# nh
abbr -a nos "nh os switch"
abbr -a nosa "nh os switch --ask"
abbr -a nob "nh os boot"
abbr -a noba "nh os boot --ask"

# nix-locate
abbr -a '?' nix-locate

# vscode
abbr -a zd zeditor
abbr -a zd. "zeditor ."

# vscode
abbr -a vc code
abbr -a vc. "code -- ."
