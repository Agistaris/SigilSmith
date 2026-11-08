# Security

If you find a security problem in SigilSmith, please tell us privately first,
so we can fix it before it is public.

## How to report

- **On GitHub:** the **Report a vulnerability** button on this repository's
  **Security** tab. Only you and we can see the report.
- **By email:** [hello@sigilsmith.com](mailto:hello@sigilsmith.com), with
  "Security" in the subject.

Please don't open a public issue, discussion or Nexus comment about it. If you
can't use either way, open an issue that says only "I have a security report"
and leave out the details, and we'll ask you to use one of the two above.

A good report says which version you use (`sigilsmith --version`), how you
installed it (AppImage, `.deb`, `.rpm` or `.tar.gz`), what happens and how to
make it happen. A small example mod or archive helps, if one is involved.
Please keep the details to yourself until a fixed release is out.

## What happens next

We reply within 14 days to say we have it, and keep you posted until it is
fixed. Our goal is a fix, or a clear reason why not, within 90 days of your
report. Both are goals, not a contract. Once a fixed release is out, we
describe the problem in the changelog and credit you, unless you'd rather not
be named. We can't offer a bounty.

## What counts

The latest release of SigilSmith: the desktop app, the terminal app and the
command line, and the release files and signatures on the releases page. For
example:

- a mod archive or file that makes SigilSmith write outside the folders listed
  in [What SigilSmith touches](docs/what-sigilsmith-touches.md);
- an update that installs without its signature and checksum matching;
- SigilSmith running a program or command it shouldn't;
- a release file that doesn't match its published checksum or signature.

Problems in the mods themselves, in the game, in Norbyte's Script Extender or
on Nexus Mods belong with their authors. If you're not sure, send it to us
anyway.

## Which versions get fixes

The newest release. An older one may be fixed too if the problem is serious
and the fix is small.

## Testing

Test on your own computer and your own files. Please don't touch other
people's data or systems. If you report in good faith, we won't take action
against you.

## Check a download yourself

Every release has `SHA256SUMS.txt` and `SHA256SUMS.txt.sig`. The signature says
the checksums come from us. SigilSmith checks it itself before it installs an
update. To check by hand you need OpenSSH. Put the release keys listed below
in a file called `allowed-signers`, put the release's `SHA256SUMS.txt` and
`SHA256SUMS.txt.sig` in the same folder as the files you downloaded, and run:

```bash
ssh-keygen -Y verify -f allowed-signers -I sigilsmith-release \
  -n sigilsmith-release -s SHA256SUMS.txt.sig < SHA256SUMS.txt
sha256sum -c --ignore-missing SHA256SUMS.txt
```

`ssh-keygen` should say `Good "sigilsmith-release" signature`, and `sha256sum`
should print `OK` next to each file you downloaded. Anything else means the
download isn't ours: don't run it, and tell us.

`ssh-keygen -Y verify` doesn't check that a security key was touched and
unlocked with its PIN; SigilSmith does. To check that by hand too, when the
signature is from the security key (it prints `touch and PIN: yes` for a good
signature; the recovery key is a software key and has neither):

```bash
n=$(sed '1d;$d' SHA256SUMS.txt.sig | base64 -d | tail -c 5 | head -c 1 | od -An -t u1)
[ $((n & 5)) -eq 5 ] && echo "touch and PIN: yes" || echo "touch and PIN: NO"
```

The release keys. Any one of them may sign a release, and a lost key is
dropped in the next release. These are the same keys SigilSmith has built in.
The contents of `allowed-signers`:

```text
sigilsmith-release sk-ecdsa-sha2-nistp256@openssh.com AAAAInNrLWVjZHNhLXNoYTItbmlzdHAyNTZAb3BlbnNzaC5jb20AAAAIbmlzdHAyNTYAAABBBCx49A9FxAJgK2nmChuJXpptSTnB24dabmArYOBQ48YGbjIpdz+5XeXUmcpwSHBD8qTs4+DOx9TXAFcjU+AOhjsAAAAWc3NoOnNpZ2lsc21pdGgtcmVsZWFzZQ==
sigilsmith-release ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILHUS0VWHU0EFt6vbJZwCCdzJ7DqcH4SrKGm5ZH/l+f+
```

Their fingerprints (the `Good "sigilsmith-release" signature` line shows the
one that signed):

| Key | Fingerprint |
| --- | --- |
| Security key (hardware, needs touch and PIN) | `SHA256:vkWFS7lTLnukaOwiQ51SSapktFyNPeW9yDpdZXM+WzI` |
| Recovery key (software) | `SHA256:7TdhJQXEX3ioNk7hZbEn4Jh1kM7/qUjihtyyHECn378` |
