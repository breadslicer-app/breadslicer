# Contributing Guidelines

## How can I contribute?

You don't have to be a developer to contribute, here is a list of ways in which
you can help this project to thrive:

- Testing alpha & beta releases
- [Contributing Knowledge](#contributing-knowledge)
- [Reporting bugs](#reporting-bugs)
- [Proposing new features or improvements](#proposing-feature-requests)
- [Improving the current documentation](#contributing-documentation)
- [Helping with translations](#contributing-translations)
- [Contributing code](#contributing-code)

## Contributing Knowledge

Creating a modern slicer for 3d printers is a complex task that requires the
work of highly skilled people. They won't be always have time to be directly
involved, but their insights might still be very useful.

If you think you can contribute with your knowledge or insights, there are
different ways for you to do that:
- We have a [Discord server](https://discord.gg/Z68pDczNv6) where you can
  participate in our chat conversations.
- We also have
  [asynchronous discussions](https://codeberg.org/SoulCrafted/Slicer/issues?q=&type=all&state=open&labels=1147840)
  that we use to gather insights before making important decisions about the
  future of the project.

## Reporting Bugs

You can report bugs through our
[issue tracker](https://codeberg.org/SoulCrafted/Slicer/issues).

Some recommendations to make your reports as useful as possible:

1. Check if someone else already reported that bug or problem, you can use the
  search feature of the issue tracker for that.
2. Be thorough and detailed with your explanations:
   - Explain how to reproduce the problem in detail. Every working environment
     is a world in itself, which sometimes makes it difficult to reproduce a bug
     without having detailed information about it.
   - Explain in detail what behavior do you expect and what happened instead.
     Sometimes, behaviour can be "underspecified", and leting know developers
     what you expect can help to clear potential misunderstandings.
   - Include context details that might influence the problem at hand: software
     version, operating system, 3d-printer model, hardware specs, STL file
     properties (size, format, ...), etc.
3. Always provide as much context info as possible: Software version, operating
  system, 3d-printer model, ...

## Proposing Feature Requests

You can propose new features through our
[issue tracker](https://codeberg.org/SoulCrafted/Slicer/issues).

Some recommendations to make your feature proposals as useful as possible:

1. Explain the "pain" that this new feature or improvement would solve
2. Describe the feature or improvement in as much detail as possible
3. Explain how do you think this feature/improvement will solve the problem that
   you detected
4. If possible, detail other alternatives that you might have considered and why
   do you think that the feature proposal is a better option

## Contributing Documentation

⚠️ **Work in Progress** ⚠️

## Contributing Translations

The translations are available in the `apps/slicer-gui/i18n/` directory.

### Translating via Web Interface (recommended)

Translations management is centralized at
https://translate.codeberg.org/projects/soulcrafted-slicer/

### Working with translation files (for developers)

> ⚠️ : Not recommended if it's not strictly necessary for technical reasons

Each language has its translations in its own directory:
```
apps/slicer-gui/i18n/[lang_code]/LC_MESSAGES/sc-slicer-gui.po
```

We can edit them with tools such as [`Poedit`](https://poedit.net/), although
one could choose a simple text editor to work on these files.

### Preparing translation files (for developers)

> ⚠️ : Not recommended if it's not strictly necessary for technical reasons

We use `slint-tr-extractor` to generate the `apps/slicer-gui/i18n/sc-slicer-gui.pot`
file by extracting translatable strings from our UI declarative files. In
general terms, translators shouldn't worry about this step, as it will be
performed by developers:
```bash
cd apps/slicer-gui/ui ;
find -name \*.slint | xargs slint-tr-extractor -o ../assets/translations/slicer-gui.pot ;
```

We can install `slint-tr-extractor` with the command:
```bash
cargo install slint-tr-extractor
```

## Contributing Code

### Basic Workflow

1. It is recommended to enable the local git hooks before commiting anything.
   Run the following command inside the repository root to enable them:
   ```bash
   git config core.hooksPath .hooks;
   ```
2. All code contributions must be proposed as
  ["Pull Requests"](https://en.wikipedia.org/wiki/Distributed_version_control#Pull_requests),
  you can find them
  [here](https://codeberg.org/SoulCrafted/Slicer/pulls).
3. Once the change is proposed, it will be reviewed by the project maintainers
   and other contributors. They might require some changes or fixes before
   approving the contribution.
4. No PRs will be reviewed until they pass the automatic checks from the CI
   pipelines.
5. Once the contribution is approved, a maintainer will merge it into the `main`
   branch.

### Editing the GUI

It is recommendable to install the tools `slint-lsp` and `slint-viewer`:
```bash
cargo install slint-lsp slint-viewer
```

We can visualize interfaces by loading the `.slint` files with `slint-viewer`:
```bash
slint-viewer ./path/to/gui-file.slint
```
