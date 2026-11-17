{
  perSystem =
    { builds, ... }:
    {
      packages.scripted-demos = builds.scripted-demos;
    };
}
