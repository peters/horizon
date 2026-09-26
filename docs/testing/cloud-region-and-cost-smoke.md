# New cloud region, stock and cost smoke

A temporary plan for checking the New cloud price card (#977, #980) and region choice
(#979) on a real display. It reads only RunPod's catalog and stock endpoints until the
last, optional section; nothing is rented unless that section is run.

## Preconditions

- A candidate build of this branch, launched with a private `HOME` so the real
  `~/.horizon` is untouched: `HOME=<scratch>/home horizon`.
- `<scratch>/home/.horizon/cloud/settings.json` with a working RunPod key file and
  `"data_centers": []`. Keep a copy with `"data_centers": ["EU-RO-1", "US-MO-2"]`
  for step 6.
- Two workspaces with a committed repository each: one whose `.horizon/cloud.yml`
  default profile is a CPU profile, one with a GPU profile listing two or three
  `gpu_types` in the settings file.
- RunPod stock changes by the minute. Where a step depends on stock, note what the
  provider reported at the time instead of expecting a fixed result.

## 1. CPU profile: prices, stock and cost

1. Select the CPU workspace, open **Cloud > New cloud**.
2. Expected: every vCPU choice reads `from $x/h` and every memory choice `$x/h`.
3. Expected: the card shows the hourly price, a stock pill, a line under the size such
   as `In stock in 2 allowed data centers`, then 8 HOURS and 24 HOURS (captioned
   `compute`), RUNNING per month (`compute and storage`) and STOPPED per month
   (`storage it keeps`).
4. Expected: the storage table lists Network volume and Container disk with sizes,
   running and stopped prices, `not billed` for the container disk while stopped,
   a sentence per storage kind and `RunPod storage list prices, checked <date>`.
5. Check the arithmetic for one size: RUNNING equals 730 × hourly + both storage rows;
   STOPPED equals the network volume row.
6. Hover the pill and the storage rows. Expected: nothing needed is hidden in a
   tooltip; all details are already on the card.

## 2. Region row (CPU)

1. With `"data_centers": []`, expected a **Region** row: `Any region` first, then
   regions in alphabetical order, each with `N in stock`, `none in stock` in red, or
   `checking stock` while the stock check runs.
2. Expected: a region with `none in stock` is dimmed and does nothing when clicked.
3. Choose a region with stock. Expected: it is highlighted, the line under the row
   reads `The workspace stays in <region>, and a stopped cloud resumes there.`, and
   the card line reads `In stock in N data centers in <region>`.
4. Choose **Any region** again. Expected: the default note and the unscoped card line
   return.
5. Change the size to one that is out of stock everywhere. Expected: every region
   reads `none in stock`, the pill reads `Out of stock`, and the card line reads
   `Out of stock in every allowed data center`.

## 3. Data center under Advanced

1. Open **Advanced** and scroll to **Data center**.
2. Expected: chips for the data centers with stock (green dot) or unknown stock (grey
   dot), each with its region below the ID, and a line counting the ones hidden
   because they have no stock.
3. Choose one. Expected: the chip is outlined in the accent color, no region button is
   highlighted, and the note reads `The workspace stays in <ID>, ...`.

## 4. GPU profile

1. Select the GPU workspace and open New cloud.
2. Expected: the Region row counts data centers where any preferred GPU is in stock.
3. Choose a region. Expected: the GPU rows' pills, the headline GPU and `Cheapest in
   stock now` follow that region.
4. Expected: when none of the preferred GPUs is in stock, the card still shows
   STOPPED and the storage table (pod volume at twice the rate while stopped).

## 5. Resets and persistence

1. Choose a region, then switch profile under Advanced. Expected: the placement
   returns to **Any region**.
2. Choose a region, change the repository. Expected: **Any region** again.
3. Choose a region and start a cloud (dry: cancel before the image build if no spend
   is wanted). Restart Horizon. Expected: the cloud card shows `Region: <region>`.

## 6. Machine setting still limits the offer

1. Replace the settings file with the copy restricting `data_centers` to
   `EU-RO-1` and `US-MO-2`. Close and reopen New cloud.
2. Expected: only regions containing those data centers appear, and the counts never
   exceed them.

## 7. Optional paid check (rents compute)

1. Start a small CPU cloud with a region chosen. Expected: the cloud becomes Ready,
   its card reads `Data center: <ID> · <region>`, and the ID is in that region.
2. Stop it, then start it. Expected: it resumes in the same data center.
3. Delete the cloud and verify the worker and volume are gone in the RunPod console.
