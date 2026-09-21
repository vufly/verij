# Hiên

**Hiên** is a workspace layer for Zellij.

The name comes from the Vietnamese word **“hiên”**, meaning a veranda or transitional space between the inside and outside of a house.

The project explores using Zellij nested sessions to add a workspace layer above Zellij's existing session → tab → pane hierarchy.

The intended architecture is roughly:

```text
Hiên
├── persistent sidebar / workspace navigation
├── global status
└── workspace
    └── nested Zellij session
        ├── tabs
        └── panes
```

Longer-term ideas include:

* workspace management
* persistent navigation/sidebar
* separate global and workspace-level status bars
* coding-agent activity monitoring
* keeping Zellij itself responsible for terminal multiplexing

The project is experimental and currently at the architecture/prototyping stage.
