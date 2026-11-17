{
  perSystem =
    { builds, ... }:
    {
      checks.lint-clippy = builds.lint-clippy;
    };
}
