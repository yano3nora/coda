coda
===

## Structure
```
.
├ docs/                 … Design notes
└ mise.toml             … Toolchain
```

## Depends
- mise 2026+

## Development
### Getting Started
```sh
# Uncomment required tools/tasks in mise.toml first.
mise install

# If you enable env / hooks / tasks, trust the config explicitly.
mise trust
mise run provision

# Dev server - http://localhost:3000
npm start
```

### Commands
```sh
# install packages
# fix / check lint, format
# type check & test
```

## Deployment
```sh
# deploy
```

### Resources
