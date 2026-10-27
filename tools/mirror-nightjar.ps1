# Publishes client/ to the nightjar repo, which stays Nightjar's public home
# (issues, releases, update checks, F-Droid and Obtainium). `git subtree split`
# turns client/'s history back into nightjar's: client/ came in with
# `git subtree add` (no squash), so the split reuses nightjar's own commits and
# extends them. The push only ever fast-forwards nightjar main; anything else
# stops before pushing.
#   tools\mirror-nightjar.ps1 -DryRun    # split, check, `git push --dry-run`
#   tools\mirror-nightjar.ps1            # the real push
param(
    [string] $Remote = 'https://github.com/RamazanKara/nightjar.git',
    [string] $Branch = 'main',
    # The monorepo commit to publish; usually main after its CI is green.
    [string] $Ref = 'HEAD',
    [switch] $DryRun
)
$ErrorActionPreference = 'Continue'

function Invoke-Git {
    $out = & git @args
    if ($LASTEXITCODE) { throw "git $($args -join ' ') failed with exit code $LASTEXITCODE" }
    $out
}

$repo = Invoke-Git -C $PSScriptRoot rev-parse --show-toplevel
$commit = Invoke-Git -C $repo rev-parse --verify "$Ref^{commit}"
# The split prints its progress on stderr and the split commit last on stdout.
$split = Invoke-Git -C $repo subtree split --prefix client $commit | Select-Object -Last 1
if ($split -notmatch '^[0-9a-f]{40}$') { throw "git subtree split printed no commit: '$split'" }
Invoke-Git -C $repo fetch --quiet --no-tags $Remote "refs/heads/$Branch" | Out-Null
$published = Invoke-Git -C $repo rev-parse --verify 'FETCH_HEAD^{commit}'
"monorepo $commit, client/ split $split, nightjar $Branch $published"
if ($split -eq $published) {
    "nightjar $Branch is up to date"
    exit 0
}
& git -C $repo merge-base --is-ancestor $published $split
if ($LASTEXITCODE -eq 1) {
    # Commits made in the nightjar repo itself (e.g. translations): merge them
    # into client/ first, then mirror again. Never force-push nightjar.
    throw "not a fast-forward: nightjar $Branch $published is not in the split. Merge it with ``git subtree pull --prefix client $Remote $Branch`` first."
}
if ($LASTEXITCODE) { throw "git merge-base failed with exit code $LASTEXITCODE" }
$count = Invoke-Git -C $repo rev-list --count "$published..$split"
"fast-forward by $count commit(s):"
Invoke-Git -C $repo log --oneline --no-decorate "$published..$split" | Select-Object -First 20
$push = @('push')
if ($DryRun) { $push += '--dry-run' }
Invoke-Git -C $repo @push $Remote "${split}:refs/heads/$Branch"
