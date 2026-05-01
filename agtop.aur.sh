# 1) SSH key for aur.archlinux.org (one-time per machine):
ssh-keygen -t ed25519 -f ~/.ssh/aur
# add ~/.ssh/aur.pub to https://aur.archlinux.org/account/

cat >> ~/.ssh/config <<EOF
Host aur.archlinux.org
  IdentityFile ~/.ssh/aur
  User aur
EOF

# 2) Clone the (empty, soon-to-be) AUR repo:
mkdir -p ~/aur && cd ~/aur
git clone ssh://aur@aur.archlinux.org/agtop.git
cd agtop

# 3) Drop in the PKGBUILD pointed at the GH release tarball:
cp ~/code/agtop/packages/pacman/PKGBUILD .
sed -i 's|^source=.*|source=("agtop-${pkgver}.tar.gz::https://github.com/mbrassey/agtop/archive/v${pkgver}.tar.gz")|' PKGBUILD

# 4) Replace SKIP with the real sha256 of the release tarball:
VER=$(awk -F'"' '/^pkgver=/{print $0}' PKGBUILD | cut -d= -f2)
curl -L -o /tmp/agtop.tar.gz "https://github.com/mbrassey/agtop/archive/v${VER}.tar.gz"
SHA=$(sha256sum /tmp/agtop.tar.gz | awk '{print $1}')
sed -i "s|^sha256sums=.*|sha256sums=('$SHA')|" PKGBUILD

# 5) Generate .SRCINFO and submit:
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO
git commit -m "agtop ${VER} — initial AUR submission"
git push
