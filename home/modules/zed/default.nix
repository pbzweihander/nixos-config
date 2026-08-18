{
  programs.zed-editor = {
    enable = true;
    userSettings = {
      format_on_save = "on";
      cli_default_open_behavior = "existing_window";
      project_panel = {
        dock = "left";
      };
      outline_panel = {
        dock = "left";
      };
      collaboration_panel = {
        dock = "left";
      };
      git_panel = {
        dock = "left";
      };
      agent = {
        default_profile = "write";
        sidebar_side = "right";
        dock = "right";
        play_sound_when_agent_done = "always";
      };
      cursor_blink = false;
      vim = {
        use_system_clipboard = "never";
      };
      helix_mode = true;
      vim_mode = false;
      base_keymap = "VSCode";
      buffer_font_family = "Sarasa Mono K";
      ui_font_family = "Sarasa UI K";
      show_wrap_guides = true;
      ui_font_size = 16;
      buffer_font_size = 15;
      theme = "Dimidium";
      wrap_guides = [
        60
        80
        100
        120
      ];
    };
    userKeymaps = [
      {
        context = "Workspace";
        unbind = {
          "ctrl-tab" = "tab_switcher::Toggle";
        };
      }
      {
        context = "Workspace";
        unbind = {
          "ctrl-shift-tab" = [
            "tab_switcher::Toggle"
            {
              select_last = true;
            }
          ];
        };
      }
      {
        context = "Workspace";
        bindings = {
          "ctrl-tab" = "vim::GoToTab";
        };
      }
      {
        context = "Workspace";
        bindings = {
          "ctrl-shift-tab" = "vim::GoToPreviousTab";
        };
      }
      {
        context = "(vim_mode == helix_normal || vim_mode == helix_select) && !menu";
        unbind = {
          "ctrl-s" = "editor::SaveLocation";
        };
      }
      {
        context = "VimControl && !menu";
        bindings = {
          "shift-b" = "editor::SelectToPreviousWordStart";
        };
      }
      {
        context = "(VimControl && !menu) || (!Editor && !Terminal)";
        bindings = {
          "ctrl-k" = "workspace::ActivatePaneUp";
        };
      }
      {
        context = "(VimControl && !menu) || (!Editor && !Terminal)";
        bindings = {
          "ctrl-j" = "workspace::ActivatePaneDown";
        };
      }
      {
        context = "(VimControl && !menu) || (!Editor && !Terminal)";
        bindings = {
          "ctrl-h" = "workspace::ActivatePaneLeft";
        };
      }
      {
        context = "(VimControl && !menu) || (!Editor && !Terminal)";
        bindings = {
          "ctrl-l" = "workspace::ActivatePaneRight";
        };
      }
    ];
    themes = {
      dimidium = ./themes/dimidium.json;
    };
  };
}
