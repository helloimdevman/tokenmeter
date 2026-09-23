# TokenMeter 릴리스

제품은 `native/` Rust 바이너리 두 개뿐이다. GitHub 정식 릴리스에만 올린다.
에셋 이름: `tokenmeter-<os>-<arch>`, `tokenmeter-hook-<os>-<arch>`, `SHA256SUMS`.
`install.sh`와 `tokenmeter update`가 이 에셋을 받아 `SHA256SUMS`와 맞을 때만 설치한다.
이름을 바꾸면 `install.sh`·`install.rs`(`platform_tag`)도 같이 바꾼다.

1. `cargo test --manifest-path native/Cargo.toml`
2. `native/meter/Cargo.toml`·`native/hook/Cargo.toml`·`native/meter/src/lib.rs`(`VERSION`)의 버전을 태그와 맞춘다
3. 태그 `vX.Y.Z`를 푸시하면 `.github/workflows/release.yml`이 linux-x64·linux-arm64·macos-arm64·macos-x64를 빌드해
   초안 릴리스에 모두 올린 뒤 마지막에 공개한다. 저장소에 변경 불가 릴리스(Immutable releases)가 켜져 있어
   공개된 릴리스에는 파일을 더 올리거나 바꿀 수 없다. 잘못 나간 릴리스는 고치지 말고 다음 버전을 낸다.
4. 히스토리에 비밀이 있으면 public 릴리스를 멈춘다

자동업데이트(`tokenmeter update on`)는 이 정식 릴리스만 받아 바이너리를 제자리에서 바꾼다.
워크플로의 액션은 커밋 SHA로 고정하고, 버전 갱신은 Dependabot PR(`.github/dependabot.yml`)로만 받는다.
macOS 코드 서명·공증은 아직 없다. Windows 빌드는 만들지 않는다.
