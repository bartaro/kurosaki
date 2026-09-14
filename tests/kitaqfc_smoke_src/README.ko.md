# KITAQFC NROM 기본 동작 확인용 소스

[English](README.md) | [日本語](README.ja.md) | **한국어**

[KUROSAKI 한국어 설명서](https://bartaro.github.io/kitaq-docs/ko/kurosaki.html) · [KITAQFC 한국어 설명서](https://bartaro.github.io/kitaq-docs/ko/kitaqfc.html)

이 소스는 향후 KITAQFC Phase26 개발 트리에서 빌드한 뒤, 컴파일러로 생성한 기준 ROM으로 `crates/kurosaki-core/tests/fixtures/`에 복사하기 위해 준비했습니다. 해당 빌드가 이미 완료되었다는 뜻은 아닙니다.

현재 테스트 파일인 `kitaqfc_nrom_smoke.nes`는 상용 게임의 코드를 사용하지 않고 바이트열을 독자적으로 생성한 ROM입니다. 이 환경에 KITAQFC나 실물이 없어도 동일한 에뮬레이터 처리 경로를 확인하기 위해 사용합니다.

아래는 향후 빌드에 사용할 명령의 초안입니다. 실행 전에 로컬 KITAQFC 버전의 CLI 옵션에 맞게 조정하세요. 현재 CLI에서 그대로 실행되는 것으로 검증된 절차는 아닙니다.

```powershell
kitaqfc.exe tests\kitaqfc_smoke_src\kitaqfc_nrom_smoke.c `
  -o crates\kurosaki-core\tests\fixtures\kitaqfc_nrom_smoke_from_kitaqfc.nes `
  --mapper nrom256 `
  --debug-out crates\kurosaki-core\tests\fixtures\debug_output
```
