# git-range-diff-better

Compare diff before/after a rebase, except better than `git range-diff` because it handles submodules
and doesn't break horribly on rebase squashes.

## Usage

```shell
cd my-git-repo
git-range-diff-better b7c14457a3d4620d2e8f9f1b03bd327f8b5a2510..5f770fe793dc4296daf9a7a89fbd59992fdc043a eb9f7c2499f9461dc3beaa9908bb1bfbbaba57f2..71a184c0806687f0f9a630ea4ea14c7853cf5ad4
```

## AI Disclosure
This repo was entirely vibe-coded by GPT-5.5 medium. No guarantees are made to its code quality.
