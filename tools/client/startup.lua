-- Test-harness replacement for Data/startup.lua (installed by tools/dev-run.sh when
-- LMO_AUTOPLAY=1). Keeps the stock scene setup, then drives the login flow the way Funcom's
-- smoke test did: wait until the client can enter the game, enter it, and log progress.
-- Log lines are tagged <AUTOPLAY> in ClientLog.txt.

function SetEnvironment()
	local sunDirection = Vec(-1, 1, 0)
	local sunColor = Vec(0.2, 0.2, 0.2)
	local sunW = 0.3
	local ambientSHR = Vec(0.01, 0.01, 0.01)
	local ambientSHG = ambientSHR
	local ambientSHB = ambientSHR
	local ambientSHW = Vec(0.2, 0.2, 0.2)
	DWEngine.SetSunEnvironment(sunDirection, sunColor, sunW, ambientSHR, ambientSHG, ambientSHB, ambientSHW)
end

DWEngine.ResetEngine()
SetEnvironment()

local function Log(msg)
	debug.log('important', "<AUTOPLAY> " .. msg)
end

DWEngine.StartCoroutine(function()
	local last = nil
	local entered = false
	while true do
		local state = string.format("canEnter=%s inGame=%s inPlay=%s disconnected=%s",
			tostring(flow.CanEnterGame()), tostring(flow.IsInGame()),
			tostring(flow.IsInPlay()), tostring(flow.IsDisconnected()))
		if state ~= last then
			Log(state)
			last = state
		end
		if not entered and flow.CanEnterGame() then
			Log("calling flow.EnterGame()")
			flow.EnterGame()
			entered = true
		end
		coroutine.yield(1)
	end
end)
