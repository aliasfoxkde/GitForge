## Pull Request

**Prefix your title with:** `feat:` `fix:` `docs:` `test:` `refactor:` `ci:` `chore:` `perf:`

_Example:_ `feat: add user authentication via OAuth`

---

### Root Cause Analysis *(required for `fix:` prefix)*

<!-- Describe the underlying cause of the issue, not just the symptom. -->

### Summary of Changes

<!-- What does this PR do? -->

### Breaking Changes

<!-- Does this PR introduce any breaking changes? If yes, describe. -->

### Testing

- [ ] `go vet ./...` passes
- [ ] `go test -p 1 ./...` passes
- [ ] `go build ./...` passes
- [ ] `golangci-lint run --timeout=5m` passes
- [ ] `gofmt -l .` shows no files
- [ ] Coverage maintained or improved

### Checklist

- [ ] Conventional commit format in title
- [ ] CODEOWNERS review required
- [ ] No new `//golint:disable` without justification
- [ ] No new debug/temporary code
- [ ] CHANGELOG updated (if applicable)
- [ ] Documentation updated (if applicable)
- [ ] No secrets or credentials committed

---

_Co-Authored-By: [Your Name](https://github.com/)_
