# Nucleus Co-op
- A background stream with Rubylight's own handler runs Nucleus Co-op without administrator rights when the game needs none, from your own copy of Nucleus Co-op in `C:\ProgramData\Butterpollo\nucleus-user\<SID>`. So it now works on a standard account and with Windows' Administrator protection on.
- A game still runs elevated when its manifest or compatibility settings ask for administrator rights, Windows treats it as an installer, or you can't change the files in its folder. Hub handlers and co-op always run elevated. On an account Windows doesn't elevate, those are refused, and the Play together panel says so before you launch.
- The Play together panel says how a background stream of the game runs: as the signed-in user, or as administrator.
