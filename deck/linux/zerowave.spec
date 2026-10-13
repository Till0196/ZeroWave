%global debug_package %{nil}
%global __os_install_post %{nil}

Name:           zerowave
Version:        %{zw_version}
Release:        1
Summary:        デジタル放送の視聴
License:        Apache-2.0
URL:            https://github.com/Till0196/zerowave
AutoReqProv:    no
Requires:       glibc >= 2.28

%description
デジタル放送（TS・MMT/TLV）を視聴するプレーヤーです。

%install
install -Dm755 %{zw_binary} %{buildroot}%{_bindir}/ZeroWave
install -Dm644 %{zw_root}/deck/linux/zerowave.desktop %{buildroot}%{_datadir}/applications/zerowave.desktop
mkdir -p %{buildroot}%{_datadir}/icons
cp -R %{zw_root}/deck/linux/icons/hicolor %{buildroot}%{_datadir}/icons/
install -Dm644 %{zw_root}/LICENSE %{buildroot}%{_datadir}/licenses/zerowave/LICENSE
install -Dm644 %{zw_root}/NOTICE %{buildroot}%{_datadir}/licenses/zerowave/NOTICE

%files
%{_bindir}/ZeroWave
%{_datadir}/applications/zerowave.desktop
%{_datadir}/icons/hicolor/*/apps/zerowave.*
%{_datadir}/licenses/zerowave
